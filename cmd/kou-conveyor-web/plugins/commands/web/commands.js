// commands: slash commands, as in the terminal cockpit. "/" lists them over
// the composer: typing filters, ↑↓ choose, Tab completes and Enter runs.
// Plugins declare commands (cockpit.commands.register, or "commands" in a
// manifest); a later plugin's command replaces an earlier one of the same
// name. Text that starts with a path, or with a space, is a prompt. It
// provides the commands service: list(), named(name), parse(raw), run(parsed).
export default function activate(cockpit) {
  const { h } = cockpit;
  const session = cockpit.use('session');
  const composer = cockpit.use('composer');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const summary = () => session.summary?.();

  // list is the commands in effect: of those with one name, the one of the
  // latest source, else the latest, in the place of the first.
  const list = () => cockpit.contributions('commands', { unique: 'name' });

  function named(name) {
    name = String(name).toLowerCase();
    return list().find((c) => c.name === name || c.aliases?.includes(name)) || null;
  }

  // shown reports whether a command applies to the view.
  function shown(command, view) {
    return !command.shown || !!cockpit.safely(() => command.shown(view));
  }

  // parse reads composer text as "/name argument", or returns null for a
  // prompt: text that starts with a space, or with a path such as /usr/bin.
  function parse(raw) {
    const match = /^\/([^\s/]*)(?:\s+([\s\S]*))?$/.exec(String(raw).trimEnd());
    if (!match) return null;
    return { name: match[1], arg: (match[2] || '').trim(), command: named(match[1]) };
  }

  // run runs a parsed command, the composer cleared first.
  function run(parsed) {
    if (!parsed.command) {
      cockpit.toast(parsed.name
        ? `Unknown command /${parsed.name}. Type / to see the commands, or start with a space to send it as a prompt.`
        : 'Type a command after /, or pick one from the list.', 'error');
      return;
    }
    composer.clear?.({ images: false });
    cockpit.safely(() => parsed.command.run(parsed.arg));
  }

  cockpit.hooks.tap('composer.submit', ({ raw }) => {
    const parsed = parse(raw);
    if (!parsed) return false;
    run(parsed);
    return true;
  }, { order: 0 });

  // ---------------------------------------------------------------- suggestions

  // moved is set once the selection was moved by hand.
  const ui = { items: [], cursor: 0, dismissed: null, moved: false };
  const listbox = h('ul', { id: 'commands', role: 'listbox', 'aria-label': 'Commands' });
  const keys = h('footer', { id: 'commands-keys' },
    h('span', null, h('kbd', { text: '↑' }), h('kbd', { text: '↓' }), ' choose'), h('span', null, h('kbd', { text: 'Tab' }), ' complete'),
    h('span', null, h('kbd', { text: '↵' }), ' run'), h('span', null, h('kbd', { text: 'Esc' }), ' close'));
  const box = h('div', { class: 'commands', id: 'commands-box', hidden: true }, listbox, keys);
  cockpit.ui.mount('composer.above', { id: 'commands', order: 10, node: box });

  function score(query, text) {
    if (!query) return 1;
    text = String(text || '').toLowerCase();
    const direct = text.indexOf(query);
    if (direct >= 0) return 100 - direct + (direct === 0 || text[direct - 1] === ' ' ? 50 : 0);
    let at = 0;
    let gaps = 0;
    for (const ch of query) {
      const next = text.indexOf(ch, at);
      if (next < 0) return 0;
      gaps += next - at;
      at = next + 1;
    }
    return Math.max(1, 40 - gaps);
  }

  // suggestions lists what the composer's text can complete to: command
  // names while the first word is typed, then the values its argument
  // takes, or a line that says what the command expects.
  function suggestions(raw) {
    if (!raw.startsWith('/') || raw.includes('\n')) return [];
    const v = summary();
    const typing = /^\/([^\s/]*)$/.exec(raw);
    if (typing) {
      const typed = typing[1].toLowerCase();
      const visible = list().filter((c) => shown(c, v));
      // Names that start with the text first, then aliases that do, then
      // names that hold it.
      const matches = [
        ...visible.filter((c) => c.name.startsWith(typed)).map((command) => ({ command })),
        ...visible.filter((c) => !c.name.startsWith(typed)).flatMap((command) => {
          const alias = typed && command.aliases?.find((a) => a.startsWith(typed));
          return alias ? [{ command, alias }] : [];
        }),
      ];
      if (typed) {
        matches.push(...visible.filter((c) => c.name.includes(typed) && !matches.some((m) => m.command === c)).map((command) => ({ command })));
      }
      // A command typed whole that does not apply now still says what it does.
      const whole = named(typed);
      if (whole && !matches.some((m) => m.command === whole)) matches.unshift({ command: whole });
      return matches;
    }
    const withArg = /^\/([^\s/]+)\s+(.*)$/.exec(raw);
    const command = withArg && named(withArg[1]);
    if (!command) return [];
    const arg = withArg[2];
    if (command.complete) {
      const query = arg.trim().toLowerCase();
      return (cockpit.safely(() => command.complete(v)) || [])
        .map((choice) => ({ choice, s: Math.max(score(query, choice.label), score(query, choice.value) * 0.8, score(query, choice.detail || '') * 0.3) }))
        .filter((x) => x.s > 0)
        .sort((a, b) => (query ? b.s - a.s : 0))
        .slice(0, 50)
        .map(({ choice }) => ({ command, choice }));
    }
    return !arg.trim() && command.args ? [{ command, hint: true }] : [];
  }

  // update shows the suggestions for the composer's text while the composer
  // has the focus, unless Esc closed them for this very text.
  function update() {
    const input = composer.input;
    if (!input) return;
    const raw = input.value;
    if (raw !== ui.dismissed) ui.dismissed = null;
    const items = document.activeElement === input && ui.dismissed === null ? suggestions(raw) : [];
    const same = items.length === ui.items.length && items.every((item, n) => item.command === ui.items[n].command && item.choice?.value === ui.items[n].choice?.value);
    ui.items = items;
    // The value in use leads while nothing is typed, so Enter keeps it.
    const current = items.findIndex((item) => item.choice?.current);
    if (!same) {
      ui.cursor = current >= 0 ? current : Math.max(0, items.findIndex((item) => !item.hint));
      ui.moved = false;
    }
    render();
  }

  function render() {
    const input = composer.input;
    const items = ui.items;
    box.hidden = !items.length;
    input?.setAttribute('aria-expanded', String(!!items.length));
    keys.hidden = !items.some((item) => !item.hint);
    if (!items.length) {
      input?.removeAttribute('aria-activedescendant');
      listbox.replaceChildren();
      return;
    }
    listbox.replaceChildren(...items.map((item, n) => {
      const { command, choice } = item;
      const selected = !item.hint && n === ui.cursor;
      const signature = choice
        ? h('span', { class: 'sig' }, h('b', { text: choice.label }))
        : h('span', { class: 'sig' },
          h('b', { text: `/${command.name}` }),
          item.alias ? h('span', { class: 'args', text: ` /${item.alias}` }) : null,
          command.args ? h('span', { class: 'args', text: ` ${command.args}` }) : null);
      return h('li', {
        role: 'option', id: `command-${n}`, class: item.hint ? 'hint' : null, 'aria-selected': String(selected),
        'aria-disabled': item.hint ? 'true' : null,
        onmousemove: () => {
          if (!item.hint && ui.cursor !== n) {
            ui.cursor = n;
            ui.moved = true;
            render();
          }
        },
        onmousedown: (event) => {
          event.preventDefault(); // the composer keeps the focus
          if (!item.hint) accept(n, true);
        },
      }, signature, h('small', { text: choice ? choice.detail || '' : command.help }));
    }));
    const current = listbox.querySelector(`#command-${ui.cursor}`);
    if (current && !items[ui.cursor]?.hint) {
      input?.setAttribute('aria-activedescendant', current.id);
      current.scrollIntoView({ block: 'nearest' });
    } else {
      input?.removeAttribute('aria-activedescendant');
    }
  }

  // accept puts a suggestion into the composer. With go, a command that
  // needs nothing more runs at once, and a value runs its command.
  function accept(n, go) {
    const item = ui.items[n];
    if (!item || item.hint) return;
    const { command, choice } = item;
    if (choice) {
      composer.set(`/${command.name} ${choice.value}`);
      if (go) composer.submit();
      return;
    }
    composer.set(`/${command.name}${command.args ? ' ' : ''}`);
    if (go && !command.args?.startsWith('<')) composer.submit();
  }

  // The keys of the suggestions, before the composer's own.
  cockpit.hooks.tap('composer.key', (event) => {
    const items = ui.items;
    if (!items.length || event.isComposing) return false;
    const choosable = items.some((item) => !item.hint);
    const step = (by) => {
      const n = items.length;
      let next = ui.cursor;
      do next = (next + by + n) % n; while (items[next].hint && next !== ui.cursor);
      ui.cursor = next;
      ui.moved = true;
      render();
    };
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation(); // not the first Esc of an Esc Esc
      ui.dismissed = composer.value();
      update();
      return true;
    }
    if (!choosable || event.altKey || event.metaKey) return false;
    const ctrl = event.ctrlKey && !event.shiftKey;
    if (event.key === 'ArrowDown' || (ctrl && event.key === 'n')) {
      event.preventDefault();
      step(1);
      return true;
    }
    if (event.key === 'ArrowUp' || (ctrl && event.key === 'p')) {
      event.preventDefault();
      step(-1);
      return true;
    }
    if (event.ctrlKey) return false;
    if (event.key === 'Tab' && !event.shiftKey) {
      event.preventDefault();
      accept(ui.cursor, false);
      return true;
    }
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      // A lone "/" has chosen nothing yet: Enter completes the command in
      // view instead of running it, unless it was picked with the arrows.
      accept(ui.cursor, ui.moved || composer.value() !== '/');
      return true;
    }
    return false;
  }, { order: 0 });

  cockpit.on('composer:input', update);
  cockpit.on('composer:focus', update);
  cockpit.on('composer:blur', update);
  cockpit.on('point:commands', update);

  cockpit.provide('commands', { list, named, parse, run, shown, suggestions });
}
