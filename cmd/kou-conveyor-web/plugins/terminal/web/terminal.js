// terminal: terminals as tabs of the sidebar ("sidebar.tab" terminal). A
// terminal's shells run on the server (terminals.go) and outlive the page;
// restty draws them (vendor/restty.esm.js — libghostty-vt in WebAssembly,
// drawn with WebGPU or WebGL2), loaded the first time a terminal shows.
//
//   ⌘D           split the pane right          (Ctrl+Alt+D elsewhere)
//   ⌘⇧D          split it down                 (Ctrl+Alt+Shift+D)
//   ⌘⌥←↑→↓       go to the pane beside
//   ⌘K  ⌘F       clear, find                   (Ctrl+Shift+K, Ctrl+F)
//   ⌘⌫           delete the word before the cursor
//   ⌃W  ⌃⇧T      close the pane, reopen it within 15s (Ctrl+Shift+W elsewhere)
//   ⌘+  ⌘−  ⌘0   the terminal's text larger, smaller, as it was
//   `            open a terminal, or go to it
//
// Keys pressed in a terminal are its own: the cockpit's other keys do not
// run there (data-keys="own"). Shells start with kou-conveyor's prompt
// theme and integration, which the Settings tab turns off (the server's
// preference), and the terminal's colours are the cockpit's (theme.js). It
// adds the terminals that run without a tab to Hello, and a section to
// Settings: the prompt theme, the font and its size.
import { createTerminal } from './view.js';

const ICON = '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M4.5 6.2 6.8 8l-2.3 1.8M8.5 10.5h3" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>';
const FONTS = [
  ['JetBrainsMonoNL-Regular.ttf', 'JetBrains Mono NL', 400, 'normal'],
  ['JetBrainsMonoNL-Bold.ttf', 'JetBrains Mono NL Bold', 700, 'normal'],
  ['JetBrainsMonoNL-Italic.ttf', 'JetBrains Mono NL Italic', 400, 'italic'],
  ['JetBrainsMonoNL-BoldItalic.ttf', 'JetBrains Mono NL Bold Italic', 700, 'italic'],
];

export default function activate(cockpit) {
  const { h, prefs } = cockpit;
  const hot = cockpit.hot.data;
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const sidebar = () => service('sidebar');
  const mac = /Mac|iPhone|iPad/.test(navigator.platform || '') || navigator.userAgentData?.platform === 'macOS';
  const keys = mac
    ? { split: '⌘D', splitDown: '⌘⇧D', find: '⌘F', clear: '⌘K', copy: '⌘C', paste: '⌘V', move: '⌘⌥←↑→↓', close: '⌃W', reopen: '⌃⇧T', zoomIn: '⌘+', zoomOut: '⌘−' }
    : { split: 'Ctrl+Alt+D', splitDown: 'Ctrl+Alt+Shift+D', find: 'Ctrl+F', clear: 'Ctrl+Shift+K', copy: 'Ctrl+C', paste: 'Ctrl+V', move: 'Ctrl+Alt+←↑→↓', close: 'Ctrl+Shift+W', reopen: 'Ctrl+Shift+T', zoomIn: 'Ctrl++', zoomOut: 'Ctrl+−' };
  // How long the shells of a terminal closed wait for ⌘⇧T, in seconds.
  const GRACE = 15;

  // ---------------------------------------------------------------- what tabs share

  const views = new Set();
  const settings = () => {
    const saved = prefs.get('terminal', null) || {};
    const size = Number(saved.fontSize);
    return { fontSize: Number.isFinite(size) && size >= 9 && size <= 28 ? size : 13, fontFamily: typeof saved.fontFamily === 'string' ? saved.fontFamily.trim() : '' };
  };
  const setSettings = (change) => {
    prefs.set('terminal', { ...settings(), ...change });
    for (const view of views) cockpit.safely(() => view.applySettings());
  };
  const fontURL = (file) => new URL(`./fonts/${file}`, import.meta.url).href;
  // The fonts come with the plugin; a font of the computer's own, when
  // chosen, comes first.
  const fonts = () => {
    const list = [];
    const family = settings().fontFamily;
    if (family) {
      list.push({ family, local: 'prefer', name: family, weight: 400, style: 'normal' });
      list.push({ family, local: 'prefer', name: `${family} Bold`, weight: 700, style: 'normal' });
    }
    for (const [file, name, weight, style] of FONTS) list.push({ url: fontURL(file), name, weight, style });
    list.push({ url: fontURL('SymbolsNerdFontMono-Regular.ttf'), name: 'Symbols Nerd Font Mono' });
    if (family) list.push({ family: 'Apple Color Emoji', local: 'prefer', name: 'Apple Color Emoji' });
    return list;
  };
  // restty is loaded once, and kept as the plugin is loaded anew.
  const restty = () => {
    hot.restty ??= import(new URL('./vendor/restty.esm.js', import.meta.url).href).catch((error) => {
      hot.restty = null;
      throw error;
    });
    return hot.restty;
  };
  // restty puts its style sheets in <style> elements, which the page's
  // policy refuses: terminal.css has them, and these markers say they are
  // there.
  const markStyles = () => {
    for (const [tag, name, value] of [['style', 'data-restty-pane-styles', '1'], ['style', 'data-restty-pane-search-ui-styles', '1'], ['meta', 'data-restty-native-scrollbar', 'true']]) {
      if (document.head.querySelector(`${tag}[${name}="${value}"]`)) continue;
      const el = document.createElement(tag);
      el.setAttribute(name, value);
      document.head.append(el);
    }
  };
  // home is the user's home folder, which the workspace's name shows as ~.
  const home = () => {
    const ws = service('session')?.currentWorkspace?.();
    if (!ws?.path || !ws.display?.startsWith('~')) return '';
    return ws.path.slice(0, ws.path.length - ws.display.length + 1);
  };
  const env = { cockpit, keys, restty, fonts, settings, markStyles, home, grace: GRACE, closed: (entry) => closed(entry), forget: (view) => views.delete(view) };

  // ---------------------------------------------------------------- closed, and back

  // What was closed lately, the latest last: a tab or a pane, whose shells
  // run on for GRACE seconds (the server ends them then), for ⌘⇧T to
  // take back.
  hot.closed ??= [];
  const recent = () => {
    hot.closed = hot.closed.filter((entry) => entry.until > Date.now());
    return hot.closed;
  };
  const undo = h('div', { class: 'term-undo', id: 'terminal-undo', hidden: true, role: 'status' });
  cockpit.ui.mount('overlays', { id: 'terminal-undo', order: 70, node: undo });
  let undoTimer = 0;
  function drawUndo() {
    clearTimeout(undoTimer);
    const list = recent();
    const last = list[list.length - 1];
    undo.hidden = !last;
    if (!last) return;
    const left = Math.max(0, Math.ceil((last.until - Date.now()) / 1000));
    undo.replaceChildren(
      h('span', { class: 'term-undo-what' }, h('b', { text: last.kind === 'pane' ? 'Pane closed' : 'Terminal closed' }), ` · ${last.title || 'Terminal'}`),
      h('span', { class: 'term-undo-left', text: `its shell ends in ${left}s` }),
      h('button', { class: 'act strong', type: 'button', onclick: () => reopen() }, `Reopen ${keys.reopen}`),
      h('button', { class: 'icon small', type: 'button', 'aria-label': 'Dismiss', onclick: () => { hot.closed = []; drawUndo(); } }, '×'));
    undoTimer = setTimeout(drawUndo, 1000 - ((Date.now() - last.at) % 1000));
  }
  cockpit.onDispose(() => clearTimeout(undoTimer));

  function closed(entry) {
    recent().push({ ...entry, at: Date.now(), until: Date.now() + GRACE * 1000 - 500 });
    drawUndo();
  }

  // reopen takes back what was closed last: its shells are kept, and it
  // shows again — a tab as a tab, a pane beside its tab's active pane.
  async function reopen() {
    const entry = recent().pop();
    drawUndo();
    if (!entry) return cockpit.toast('Nothing closed lately to reopen');
    const kept = [];
    for (const id of entry.terminals) {
      try {
        await cockpit.api(`/api/terminals/${encodeURIComponent(id)}/keep`, { method: 'POST' });
        kept.push(id);
      } catch { /* it ended */ }
    }
    if (!kept.length) return cockpit.toast('Its shell already ended', 'error');
    const bar = sidebar();
    if (!bar) return undefined;
    if (entry.kind === 'pane') {
      // Back into its tab, where it was in it.
      const home = bar.tabs().find((t) => t.id === entry.tab);
      if (home) {
        bar.activate(home.id);
        const view = bar.view(home.id);
        if (view && await view.adopt(kept[0], entry.cwd, entry.placement)) return undefined;
      }
      return bar.open('terminal', { id: entry.tab, state: { layout: { t: 'pane', id: kept[0], cwd: entry.cwd } } });
    }
    // A tab comes back as it was: its ID — the panes closed from it before
    // find it — its place and its layout.
    return bar.open('terminal', { id: entry.tab, index: entry.index, state: { layout: entry.layout, fontSize: entry.fontSize || undefined }, title: entry.title });
  }



  cockpit.contribute('sidebar.tab', {
    id: 'terminal', title: 'Terminal', icon: ICON, order: 20, key: '`',
    description: `A shell in the workspace that runs on the server and outlives the page. ${keys.split} splits it right, ${keys.splitDown} down.`,
    create(tab) {
      const view = createTerminal(env, tab);
      views.add(view);
      return view;
    },
  });
  cockpit.onDispose(() => { for (const view of [...views]) cockpit.safely(() => view.dispose()); });

  // ---------------------------------------------------------------- finding a terminal

  // viewOf is the terminal an event is in, else the one the sidebar shows.
  function viewOf(event) {
    const target = event?.target;
    if (target instanceof Node) for (const view of views) if (view.contains(target)) return view;
    const active = sidebar()?.active?.();
    if (!active || active.kind !== 'terminal' || !sidebar().visible(active.id)) return null;
    // Keys typed elsewhere are theirs.
    if (target instanceof Element && target.closest('input, textarea, select, [contenteditable="true"]')) return null;
    return sidebar().view(active.id);
  }

  // openTerminal goes to a terminal, or opens one.
  function openTerminal(fresh = false) {
    const bar = sidebar();
    if (!bar) return cockpit.toast('The terminal needs the sidebar plugin, which is off.', 'error');
    return bar.open('terminal', { reuse: !fresh });
  }

  // ---------------------------------------------------------------- keys

  const primary = (event) => (mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey);
  cockpit.keys.register({
    key: ['Mod+d', 'Mod+Alt+d', 'Mod+в', 'Mod+Alt+в'], global: true, own: true, priority: 60, repeat: false,
    when: (event) => primary(event) && (mac ? !event.altKey : event.altKey) && !!viewOf(event),
    run: (event) => {
      event.stopPropagation();
      viewOf(event).split(event.shiftKey ? 'horizontal' : 'vertical');
    },
  });
  cockpit.keys.register({
    key: ['Mod+k', 'Mod+л'], global: true, own: true, priority: 60,
    when: (event) => (mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && event.shiftKey) && [...views].some((view) => view.contains(event.target)),
    run: (event) => {
      event.stopPropagation();
      viewOf(event).clear();
    },
  });
  for (const [arrow, direction] of [['ArrowLeft', 'left'], ['ArrowRight', 'right'], ['ArrowUp', 'up'], ['ArrowDown', 'down']]) {
    cockpit.keys.register({
      key: `Mod+Alt+${arrow}`, global: true, own: true, priority: 60,
      when: (event) => primary(event) && [...views].some((view) => view.contains(event.target)),
      run: (event) => {
        event.stopPropagation();
        viewOf(event).move(direction);
      },
    });
  }
  // ⌃W closes the terminal in focus (its pane, when split), ⌃⇧T takes it
  // back: browsers keep ⌘W and ⌘⇧T for their own tabs. ⌃W no longer
  // reaches the shell, which ⌘⌫ sends it for.
  cockpit.keys.register({
    key: ['Mod+w', 'Mod+ц'], global: true, own: true, priority: 60,
    when: (event) => event.ctrlKey && !event.metaKey && !event.altKey && (mac ? !event.shiftKey : event.shiftKey) && !!viewOf(event),
    run: (event) => {
      event.stopPropagation();
      viewOf(event).closeActive();
    },
  });
  cockpit.keys.register({
    key: ['Mod+t', 'Mod+е'], global: true, own: true, priority: 60,
    when: (event) => event.ctrlKey && !event.metaKey && event.shiftKey && !event.altKey && recent().length > 0,
    run: (event) => {
      event.stopPropagation();
      reopen();
    },
  });
  // ⌘+ and ⌘− make the text of the terminal in focus larger and smaller,
  // ⌘0 gives it its size back; elsewhere they zoom the page, as ever.
  const inTerminal = (event) => (mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey) && !event.altKey &&
    [...views].some((view) => view.contains(event.target));
  for (const [list, step] of [[['Mod+=', 'Mod++'], 1], [['Mod+-', 'Mod+_'], -1], [['Mod+0'], 0]]) {
    cockpit.keys.register({
      key: list, global: true, own: true, priority: 60, when: inTerminal,
      run: (event) => {
        event.stopPropagation();
        viewOf(event).zoom(step);
      },
    });
  }
  // ⌘⌫ deletes the word before the cursor (^W), as in macOS text fields.
  cockpit.keys.register({
    key: 'Mod+Backspace', global: true, own: true, priority: 60,
    when: (event) => mac && event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey && [...views].some((view) => view.contains(event.target)),
    run: (event) => {
      event.stopPropagation();
      viewOf(event).deleteWord();
    },
  });
  cockpit.keys.register({ key: ['`', 'ё'], views: ['sessions'], run: () => openTerminal() });
  if (mac) cockpit.contribute('help.keys', { keys: ['⌘', '⌫'], text: 'Delete the word before the cursor', order: 219.9 });
  cockpit.contribute('help.keys', { keys: mac ? ['⌃', 'W'] : ['Ctrl', '⇧', 'W'], text: 'Close the terminal pane', order: 219.6 });
  cockpit.contribute('help.keys', { keys: mac ? ['⌃', '⇧', 'T'] : ['Ctrl', '⇧', 'T'], text: `Reopen it, within ${GRACE}s`, order: 219.7 });
  cockpit.contribute('help.keys', { keys: mac ? ['⌘', '+ −'] : ['Ctrl', '+ −'], text: 'Terminal text larger / smaller (⌘0 back)', order: 219.8 });
  cockpit.contribute('help.keys', { keys: ['`'], text: 'Terminal', order: 217 });
  cockpit.contribute('help.keys', { keys: mac ? ['⌘', 'D'] : ['Ctrl', 'Alt', 'D'], text: 'Split the terminal right', order: 218 });
  cockpit.contribute('help.keys', { keys: mac ? ['⌘', '⇧', 'D'] : ['Ctrl', 'Alt', '⇧', 'D'], text: 'Split the terminal down', order: 219 });
  cockpit.contribute('help.keys', { keys: mac ? ['⌘', '⌥', '←→'] : ['Ctrl', 'Alt', '←→'], text: 'Go to the pane beside', order: 219.5 });

  cockpit.commands.register({
    name: 'terminal', args: '[new]', help: 'Go to a terminal, or open one: new opens another', order: 216,
    complete: () => [{ value: 'new', label: 'new', detail: 'Another terminal' }],
    run: (arg) => openTerminal(String(arg || '').trim() === 'new'),
  });
  cockpit.contribute('palette.provider', {
    id: 'terminal', order: 158,
    items: () => {
      const list = [{ group: 'Terminal', icon: '❯', label: 'New terminal', hint: '`', order: 158, run: () => openTerminal(true) }];
      if (recent().length) list.push({ group: 'Terminal', icon: '↶', label: 'Reopen the closed terminal', hint: keys.reopen, order: 158.5, run: reopen });

      const view = viewOf(null);
      if (view) {
        list.push({ group: 'Terminal', icon: '▯', label: 'Split the terminal right', hint: keys.split, order: 159, run: () => view.split('vertical') });
        list.push({ group: 'Terminal', icon: '▭', label: 'Split the terminal down', hint: keys.splitDown, order: 160, run: () => view.split('horizontal') });
        list.push({ group: 'Terminal', icon: '⌫', label: 'Clear the terminal', hint: keys.clear, order: 161, run: () => view.clear() });
        list.push({ group: 'Terminal', icon: '×', label: 'Close the terminal pane', hint: keys.close, order: 161.5, run: () => view.closeActive() });
      }
      return list;
    },
  });

  // The terminals follow the cockpit's light and dark.
  cockpit.on('theme', () => { for (const view of views) cockpit.safely(() => view.applyTheme()); });
  const scheme = matchMedia('(prefers-color-scheme: light)');
  cockpit.listen(scheme, 'change', () => { for (const view of views) cockpit.safely(() => view.applyTheme()); });

  // ---------------------------------------------------------------- Hello

  // The workspace's shells no tab shows: after the page's storage was
  // cleared, or from another browser.
  cockpit.contribute('sidebar.hello', {
    id: 'terminals', order: 20,
    render() {
      const box = h('section', { class: 'hello-section', hidden: true });
      cockpit.api('/terminals').then(({ terminals }) => {
        const shown = new Set();
        for (const t of sidebar()?.tabs?.() || []) {
          const walk = (node) => {
            if (!node) return;
            if (node.t === 'pane' && node.id) shown.add(node.id);
            walk(node.a);
            walk(node.b);
          };
          if (t.kind === 'terminal') walk(t.state?.layout);
        }
        for (const view of views) for (const id of view.terminals()) shown.add(id);
        const loose = (terminals || []).filter((t) => !t.exited && !shown.has(t.id));
        if (!loose.length) return;
        const root = home();
        box.replaceChildren(
          h('div', { class: 'label' }, h('span', { text: 'Shells without a tab' }), h('span', { text: String(loose.length) })),
          h('div', { class: 'hello-rows' }, loose.map((t) => h('div', { class: 'hello-row' },
            h('span', { class: 'what' },
              h('b', { text: t.title || (root && t.cwd?.startsWith(root) ? `~${t.cwd.slice(root.length)}` : t.cwd) || t.shell }),
              h('small', { text: [t.running ? `● ${t.running}` : t.shell.split('/').pop(), t.closing_at ? `ends in ${Math.max(0, Math.round((Date.parse(t.closing_at) - Date.now()) / 1000))}s` : `started ${cockpit.fmt.ago(t.started_at)}`, t.clients ? 'shown elsewhere' : ''].filter(Boolean).join(' · ') })),
            h('span', { class: 'ins-actions' },
              h('button', {
                class: 'act strong', type: 'button',
                onclick: async () => {
                  if (t.closing_at) await cockpit.api(`/api/terminals/${encodeURIComponent(t.id)}/keep`, { method: 'POST' }).catch(() => {});
                  hot.closed = hot.closed.filter((entry) => !entry.terminals.includes(t.id));
                  drawUndo();
                  sidebar()?.open('terminal', { state: { layout: { t: 'pane', id: t.id, cwd: t.cwd } } });
                },
              }, 'Show'),
              h('button', {
                class: 'act', type: 'button', title: 'End the shell',
                onclick: (event) => {
                  cockpit.api(`/api/terminals/${encodeURIComponent(t.id)}`, { method: 'DELETE' }).catch((error) => cockpit.toast(error.message, 'error'));
                  event.currentTarget.closest('.hello-row')?.remove();
                },
              }, 'End'))))));
        box.hidden = false;
      }).catch(() => {});
      return box;
    },
  });

  // ---------------------------------------------------------------- Settings

  cockpit.contribute('settings.section', {
    id: 'terminal', title: 'Terminal', order: 20,
    render() {
      const choice = (value, title, text) => h('label', null,
        h('input', { type: 'radio', name: 'terminal-theme', value, onchange: () => saveTheme(value) }),
        h('span', null, h('b', { text: title }), h('small', { text })));
      const themes = h('div', { class: 'set-choice', role: 'radiogroup', 'aria-label': 'Prompt theme' },
        choice('kou', 'kou-conveyor', 'The shell runs your own startup files unchanged, then takes kou-conveyor\'s prompt and tells the terminal where it is. Only in these terminals: nothing is written to your ~/.zshrc.'),
        choice('shell', 'Your shell\'s own', 'The shell starts as in any other terminal, with your prompt, and nothing of kou-conveyor\'s runs in it.'));
      const note = h('p', { class: 'set-note' });
      const restart = h('button', { class: 'act', type: 'button', hidden: true, onclick: (event) => {
        event.currentTarget.hidden = true;
        for (const view of views) cockpit.safely(() => view.restartAll());
      } }, 'Restart open shells');
      let current = '';
      const draw = (data) => {
        current = data.theme;
        for (const input of themes.querySelectorAll('input')) input.checked = input.value === data.theme;
        const shell = (data.shell || '').split('/').pop();
        note.replaceChildren(
          'The shell is ', h('code', { text: data.shell || 'unknown' }), ' ($SHELL). ',
          data.integration ? `kou-conveyor's theme works in zsh, bash and fish. ` : `kou-conveyor's theme needs zsh, bash or fish: ${shell} starts as it is. `,
          'A change applies to shells started after it.');
      };
      cockpit.api('/api/terminal/settings').then(draw, (error) => note.replaceChildren(error.message));
      async function saveTheme(theme) {
        if (theme === current) return;
        try {
          draw(await cockpit.api('/api/terminal/settings', { method: 'PUT', body: { theme } }));
          cockpit.toast(theme === 'kou' ? 'New shells start with kou-conveyor\'s prompt' : 'New shells start with your own prompt');
          restart.hidden = views.size === 0;
        } catch (error) {
          cockpit.toast(error.message, 'error');
        }
      }

      const size = h('input', {
        type: 'number', min: '9', max: '28', step: '1', value: String(settings().fontSize), 'aria-label': 'Font size',
        onchange: (event) => {
          const value = Math.min(28, Math.max(9, Math.round(Number(event.currentTarget.value) || 13)));
          event.currentTarget.value = String(value);
          setSettings({ fontSize: value });
        },
      });
      const step = (by) => {
        size.value = String(Math.min(28, Math.max(9, settings().fontSize + by)));
        setSettings({ fontSize: Number(size.value) });
      };
      const family = h('input', { type: 'text', value: settings().fontFamily, placeholder: 'JetBrains Mono (bundled)', spellcheck: 'false', 'aria-label': 'Font family' });
      const useFamily = async () => {
        const value = family.value.trim();
        // A font of the computer's own needs the browser's leave to read
        // the computer's fonts, asked for now, while the click counts.
        if (value && typeof window.queryLocalFonts === 'function') {
          try {
            const found = await window.queryLocalFonts();
            if (!found.some((font) => `${font.family} ${font.fullName}`.toLowerCase().includes(value.toLowerCase()))) {
              cockpit.toast(`No font named "${value}" on this computer`, 'error');
              return;
            }
          } catch (error) {
            cockpit.toast(`The browser did not let the terminal read the computer's fonts: ${error.message}`, 'error');
            return;
          }
        } else if (value) {
          cockpit.toast('This browser cannot read the computer\'s fonts; the bundled font stays.', 'error');
        }
        setSettings({ fontFamily: value });
      };
      return h('div', null,
        h('div', { class: 'set-row' }, h('span', { class: 'label', text: 'Prompt theme' }), themes, note, restart),
        h('div', { class: 'set-row' }, h('span', { class: 'label', text: 'Font size' }),
          h('div', { class: 'set-inline' },
            h('button', { class: 'act', type: 'button', 'aria-label': 'Smaller', onclick: () => step(-1) }, '−'),
            size,
            h('button', { class: 'act', type: 'button', 'aria-label': 'Larger', onclick: () => step(1) }, '+'))),
        h('div', { class: 'set-row' }, h('span', { class: 'label', text: 'Font' }),
          h('div', { class: 'set-inline' }, family,
            h('button', { class: 'act', type: 'button', onclick: useFamily }, 'Use'),
            h('button', { class: 'act', type: 'button', onclick: () => { family.value = ''; setSettings({ fontFamily: '' }); } }, 'Bundled')),
          h('p', { class: 'set-note', text: 'A font installed on this computer, such as a Nerd Font your prompt needs; empty for the bundled JetBrains Mono with Nerd Font symbols.' })));
    },
  });
}
