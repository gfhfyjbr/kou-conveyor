// changes: a side panel (layout.panel "changes") with what the prompt in
// view changed, file by file: the files as a tree above, the diff of one
// below. Scrolling to another prompt shows its changes. The first file
// shows first; while a run goes on, the file it changed last does, until a
// file is picked.
const ICON = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 2.5v5M1.5 5h5M9.5 11h5" stroke="currentColor" stroke-width="1.4"/><path d="M2 13.5 14 2.5" stroke="currentColor" stroke-width="1.1" opacity=".55"/></svg>';
const STATUS_LETTER = { added: 'A', modified: 'M', deleted: 'D', renamed: 'R', typechange: 'T' };

export default function activate(cockpit) {
  const { h, svg } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();
  const layout = () => service('layout');
  const hot = cockpit.hot.data;
  const changes = hot.changes ??= {
    view: null, // the view the panel shows a prompt of
    message: null, // that prompt, by message ID
    data: null, // the server's listing of its files
    selected: null, // the file shown
    follow: true, // show the file changed last, while the run goes on
    ticket: 0,
    diffTicket: 0,
    diffs: new Map(), // `${message} ${tree} ${path}` → { patch, truncated } or { error }
    collapsed: new Set(), // folders folded in the tree
  };
  changes.view = null; // look at what is in view again
  let refresh = 0;
  let frame = 0;
  cockpit.onDispose(() => {
    clearTimeout(refresh);
    cancelAnimationFrame(frame);
  });

  const scope = h('span', { class: 'changes-scope', id: 'changes-scope' });
  const live = h('span', { class: 'changes-live', id: 'changes-live', hidden: true }, h('i', { 'aria-hidden': 'true' }), 'Live');
  const stat = h('span', { class: 'changes-stat', id: 'changes-stat' });
  const follow = h('button', {
    class: 'act', id: 'changes-follow', type: 'button', title: 'Show each file as the agent changes it', hidden: true,
    onclick: () => { changes.follow = true; load(); },
  }, 'Follow');
  const tree = h('nav', { class: 'changes-tree', id: 'changes-tree', 'aria-label': 'Changed files' });
  const diff = h('section', { class: 'changes-diff', id: 'changes-diff', 'aria-label': 'Diff' });
  const node = h('aside', { class: 'changes', id: 'changes', 'aria-label': 'Changes' },
    h('header', { class: 'changes-head' }, h('span', { class: 'label', text: 'Changes' }), scope, live, stat, follow,
      h('button', { class: 'icon', id: 'changes-close', type: 'button', 'aria-label': 'Close changes', onclick: () => layout()?.closePanel?.('changes') }, '×')),
    h('div', { class: 'changes-body' }, tree, diff));
  cockpit.contribute('layout.panel', {
    // A diff wants room: the panel is made no narrower than minWidth.
    id: 'changes', order: 10, width: 'var(--changes)', minWidth: 320, node,
    opened: () => { changes.view = null; sync(); },
  });

  const button = h('button', { class: 'icon', id: 'changes-toggle', type: 'button', title: 'Changes (D)', 'aria-label': 'Toggle changes', 'aria-pressed': 'false', onclick: () => toggle() }, svg(ICON));
  cockpit.ui.mount('bar.end', { id: 'changes-toggle', order: 60, node: button });

  const open = () => !!layout()?.panelOpen?.('changes');
  const toggle = () => layout()?.togglePanel?.('changes');
  cockpit.on('layout:panel', () => {
    button.setAttribute('aria-pressed', String(open()));
    if (open()) {
      changes.view = null;
      sync();
    }
  });

  function schedule() {
    if (!open() || frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      sync();
    });
  }

  // runPrompt is the prompt whose run a prompt belongs to, under which its
  // changes are recorded: a prompt forced in while the agent worked is
  // part of the run of the prompt before it.
  function runPrompt(v, prompt) {
    if (!prompt?.forced) return prompt;
    for (let i = v.order.indexOf(prompt.id) - 1; i >= 0; i--) {
      const entry = v.entries.get(v.order[i]);
      if (entry?.kind === 'user' && !entry.forced) return entry;
    }
    return prompt;
  }

  // sync shows the changes of the prompt whose run is in view.
  function sync() {
    if (!open()) return;
    const v = view();
    if (!v) return;
    const prompt = runPrompt(v, service('timeline')?.promptInView?.(v) || session.lastPrompt(v));
    const message = prompt ? prompt.id.replace(/^input:/, '') : null;
    if (changes.view === v && changes.message === message) return;
    changes.view = v;
    changes.message = message;
    changes.data = null;
    changes.selected = null;
    changes.follow = true;
    diff.scrollTop = 0;
    render();
    if (message) load();
  }

  const path = (v, message, rest = '') => cockpit.wsPath(v.ws, `/sessions/${encodeURIComponent(v.id)}/changes/${encodeURIComponent(message)}${rest}`);

  async function load() {
    const v = changes.view;
    const message = changes.message;
    if (!v || !message) return;
    const ticket = ++changes.ticket;
    let data;
    try {
      data = await cockpit.api(path(v, message));
    } catch (error) {
      if (ticket === changes.ticket) {
        changes.data = { available: false, reason: error.message };
        render();
      }
      return;
    }
    if (ticket !== changes.ticket) return;
    changes.data = data;
    const order = fileOrder(data.files || []);
    const latest = (data.latest || []).find((file) => order.includes(file));
    if (data.live && changes.follow && latest) changes.selected = latest;
    else if (!order.includes(changes.selected)) changes.selected = order[0] || null;
    render();
    loadDiff();
  }

  async function loadDiff() {
    const { view: v, message, data, selected } = changes;
    const file = data?.files?.find((f) => f.path === selected);
    if (!file || file.binary) return;
    const key = `${message} ${data.tree} ${file.path}`;
    if (changes.diffs.has(key)) return renderDiff();
    const ticket = ++changes.diffTicket;
    let answer;
    try {
      const old = file.old_path ? `&old=${encodeURIComponent(file.old_path)}` : '';
      answer = await cockpit.api(path(v, message, `/diff?path=${encodeURIComponent(file.path)}${old}`));
    } catch (error) {
      answer = { error: error.message };
    }
    if (ticket !== changes.diffTicket) return;
    changes.diffs.set(key, answer);
    // Old diffs go first; a live run makes many.
    if (changes.diffs.size > 200) changes.diffs.delete(changes.diffs.keys().next().value);
    return renderDiff();
  }

  function pick(file) {
    if (changes.selected !== file) diff.scrollTop = 0;
    changes.selected = file;
    // Picking a file stops following the run's changes, until Follow.
    if (changes.data?.live) changes.follow = false;
    render();
    loadDiff();
  }

  // The tree: folders first, then files, by name; a folder that holds only
  // another folder shares its row.
  function buildTree(files) {
    const root = { dirs: new Map(), files: [] };
    for (const file of files) {
      let at = root;
      for (const part of file.path.split('/').slice(0, -1)) {
        if (!at.dirs.has(part)) at.dirs.set(part, { name: part, dirs: new Map(), files: [] });
        at = at.dirs.get(part);
      }
      at.files.push(file);
    }
    return root;
  }

  function walkTree(at, prefix, depth, out, folds) {
    for (let dir of [...at.dirs.values()].sort((a, b) => a.name.localeCompare(b.name))) {
      let name = dir.name;
      let full = prefix ? `${prefix}/${dir.name}` : dir.name;
      while (!dir.files.length && dir.dirs.size === 1) {
        const [only] = dir.dirs.values();
        name += `/${only.name}`;
        full += `/${only.name}`;
        dir = only;
      }
      out.push({ dir: true, name, path: full, depth });
      if (!folds?.has(full)) walkTree(dir, full, depth + 1, out, folds);
    }
    for (const file of [...at.files].sort((a, b) => a.path.localeCompare(b.path))) out.push({ file, depth });
    return out;
  }

  // fileOrder lists the files as the unfolded tree shows them.
  function fileOrder(files) {
    return walkTree(buildTree(files), '', 0, [], null).filter((row) => row.file).map((row) => row.file.path);
  }

  function counts(added, removed) {
    return [h('b', { class: 'plus', text: `+${added}` }), ' ', h('b', { class: 'minus', text: `−${removed}` })];
  }

  // What the tree and the diff show, to know when a new snapshot changes
  // them: the tree's rows, and the diff's prompt, file and content.
  let treeDrawn = null;
  let diffShown = null; // { message, path, sig }, or null for a note
  let unselected = null; // stops waiting for a selection in the diff to go

  function render() {
    if (!open()) return;
    const { data, message } = changes;
    const n = message ? view()?.users.get(`input:${message}`) : 0;
    scope.textContent = n ? `Prompt ${String(n).padStart(2, '0')}` : '';
    live.hidden = !data?.live;
    follow.hidden = !(data?.live && !changes.follow);
    const numbers = data?.stat;
    stat.replaceChildren(...(numbers?.files ? [...counts(numbers.added, numbers.removed), ` · ${numbers.files} ${numbers.files === 1 ? 'file' : 'files'}`] : []));

    const listed = data?.available && data.files.length ? walkTree(buildTree(data.files), '', 0, [], changes.collapsed) : [];
    const latest = new Set(data?.live ? data.latest || [] : []);
    // A snapshot that leaves the rows as they are leaves the tree be.
    const key = JSON.stringify(listed.map((row) => (row.dir
      ? [row.path, row.name, row.depth, changes.collapsed.has(row.path)]
      : [row.file.path, row.file.old_path, row.file.status, row.file.binary, row.file.added, row.file.removed, row.depth, latest.has(row.file.path), row.file.path === changes.selected])));
    if (key !== treeDrawn) {
      treeDrawn = key;
      const rows = [];
      for (const row of listed) {
        let el;
        if (row.dir) {
          const folded = changes.collapsed.has(row.path);
          el = h('button', {
            type: 'button', class: 'ct-row ct-dir', 'aria-expanded': String(!folded), title: row.path, data: { key: `d:${row.path}` },
            onclick: () => {
              if (folded) changes.collapsed.delete(row.path);
              else changes.collapsed.add(row.path);
              render();
            },
          }, h('span', { class: 'chev', 'aria-hidden': 'true' }), h('span', { class: 'ct-name', text: `${row.name}/` }));
        } else {
          const f = row.file;
          el = h('button', {
            type: 'button', class: `ct-row${latest.has(f.path) ? ' latest' : ''}`, 'aria-current': String(f.path === changes.selected),
            title: f.old_path ? `${f.old_path} → ${f.path}` : f.path, data: { key: `f:${f.path}` }, onclick: () => pick(f.path),
          },
          h('span', { class: `ct-st ${f.status}`, title: f.status, text: STATUS_LETTER[f.status] || '?' }),
          h('span', { class: 'ct-name', text: f.path.split('/').pop() }),
          h('span', { class: 'ct-n' }, f.binary ? 'binary' : counts(f.added, f.removed)));
        }
        // Inline styles are refused by the page's policy; properties are not.
        el.style.setProperty('--depth', String(row.depth));
        rows.push(el);
      }
      // The row with the focus is drawn anew: the focus goes on to its new row.
      const had = tree.contains(document.activeElement) ? document.activeElement.dataset.key : null;
      tree.replaceChildren(...rows);
      if (had) tree.querySelector(`.ct-row[data-key="${CSS.escape(had)}"]`)?.focus({ preventScroll: true });
    }
    renderDiff();
  }

  const note = (text) => h('p', { class: 'changes-note', text });

  // show puts what the diff shows in, and remembers what it is.
  function show(what, ...nodes) {
    diffShown = what;
    diff.replaceChildren(...nodes);
  }

  // selectedIn says whether text in an element is selected.
  function selectedIn(el) {
    const selection = document.getSelection?.();
    return !!selection?.rangeCount && !selection.isCollapsed && selection.getRangeAt(0).intersectsNode(el);
  }

  // redrawUnselected draws the diff again once no text in it is selected.
  function redrawUnselected() {
    if (unselected) return;
    unselected = cockpit.listen(document, 'selectionchange', () => {
      if (selectedIn(diff)) return;
      unselected();
      unselected = null;
      renderDiff();
    });
  }

  function renderDiff() {
    const { data, message } = changes;
    if (!message) return show(null, note('What the agent changes shows here: each prompt\'s changes, file by file, as the transcript scrolls to it.'));
    if (!data) return show(null, note('Loading…'));
    if (!data.available) return show(null, note(data.reason || 'No changes were recorded for this prompt.'));
    if (!data.files.length) return show(null, note(data.live ? 'No files changed yet.' : 'This prompt changed no files.'));
    const file = data.files.find((f) => f.path === changes.selected);
    if (!file) return show(null);
    // A new snapshot of the file in view: its diff stays, scrolled where it
    // is, until the new one is in, and is drawn again only if it differs.
    const same = diffShown?.message === message && diffShown.path === file.path;
    const head = h('div', { class: 'diff-head' },
      h('span', { class: 'path', title: file.path, text: file.path }),
      file.old_path && h('span', { class: 'from', text: `from ${file.old_path}` }),
      file.binary ? null : h('span', null, counts(file.added, file.removed)));
    if (file.binary) return show(null, head, note('A binary file; its content is not shown.'));
    const answer = changes.diffs.get(`${message} ${data.tree} ${file.path}`);
    if (!answer) return same ? undefined : show(null, head, note('Loading…'));
    if (answer.error) return show(null, head, note(answer.error));
    const sig = [file.old_path || '', file.added, file.removed, answer.truncated ? 1 : 0, answer.patch || ''].join('\n');
    if (same && diffShown.sig === sig) return;
    // Text selected in it stays selected: the new diff is drawn once
    // nothing in it is.
    if (same && selectedIn(diff)) return redrawUnselected();
    const top = diff.scrollTop;
    const left = diff.scrollLeft;
    show({ message, path: file.path, sig }, head, diffBody(answer.patch || '', answer.truncated, file));
    if (same) {
      diff.scrollTop = top;
      diff.scrollLeft = left;
    }
  }

  // diffBody renders a unified diff with the lines' numbers, old and new.
  function diffBody(patch, truncated, file) {
    const rows = [];
    let oldLine = 0;
    let newLine = 0;
    let inHunk = false;
    const row = (kind, a, b, sign, text) => h('div', { class: `dl ${kind}` },
      h('span', { class: 'n', text: a }), h('span', { class: 'n', text: b }), h('span', { class: 's', text: sign }), h('span', { class: 'c', text }));
    for (const line of patch.split('\n')) {
      const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(line);
      if (hunk) {
        [oldLine, newLine, inHunk] = [+hunk[1], +hunk[2], true];
        rows.push(h('div', { class: 'dl hunk' }, h('span', { class: 'c', text: line })));
      } else if (line.startsWith('diff --git ')) {
        inHunk = false; // another file's headers follow
      } else if (!inHunk) {
        continue; // the patch's own headers; the panel has its own
      } else if (line.startsWith('+')) {
        rows.push(row('add', '', String(newLine++), '+', line.slice(1)));
      } else if (line.startsWith('-')) {
        rows.push(row('del', String(oldLine++), '', '−', line.slice(1)));
      } else if (line.startsWith(' ')) {
        rows.push(row('', String(oldLine++), String(newLine++), '', line.slice(1)));
      } else if (line.startsWith('\\')) {
        rows.push(h('div', { class: 'dl meta' }, h('span', { class: 'c', text: line.slice(2) })));
      }
    }
    if (!rows.length) {
      return note(file.status === 'renamed' ? 'Renamed; the content is the same.' : 'Only the file\'s mode changed.');
    }
    if (truncated) rows.push(note('The diff goes on; the rest is not shown.'));
    return h('div', { class: 'diff' }, rows);
  }

  // ---------------------------------------------------------------- following the session

  cockpit.on('timeline:flush', schedule);
  cockpit.on('timeline:scroll', schedule);
  cockpit.on('session:view', schedule);
  // A snapshot's news from a run's stream.
  cockpit.on('session:changes', (v, update) => {
    if (!open() || v !== changes.view || update?.message !== changes.message) return;
    clearTimeout(refresh);
    refresh = setTimeout(load, 120);
  });
  // The run's last snapshot is in; the panel is no longer live.
  cockpit.on('session:finish', (v) => { if (open() && changes.view === v) load(); });

  cockpit.provide('changes', { toggle, open, sync });
  cockpit.commands.register({ name: 'changes', help: 'Show or hide what the prompt in view changed', order: 210, run: toggle });
  cockpit.keys.register({ key: 'd', views: ['sessions'], run: toggle });
  cockpit.contribute('help.keys', { keys: ['D'], text: 'Changes of the prompt in view', order: 210 });
  cockpit.contribute('palette.provider', {
    id: 'changes', order: 150,
    items: () => [{ group: 'Actions', icon: '±', label: open() ? 'Hide changes' : 'Show changes', hint: 'D', order: 150, run: toggle }],
  });
  button.setAttribute('aria-pressed', String(open()));
  if (open()) sync();
}
