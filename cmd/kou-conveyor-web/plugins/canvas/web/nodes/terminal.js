// The body of a terminal node: its shell — a plain one, a command, or a
// harness such as Claude Code — drawn live by the terminal plugin's
// service while the node is in view and near enough to read (lod.js), and
// as a snapshot of its screen otherwise. A shell that is not running says
// why, and offers to start it again. Without the terminal plugin the
// snapshot stays, with a field that sends a line to the shell.

const RUNNING = new Set(['starting', 'idle', 'busy', 'waiting']);

export function createTerminalBody(node, ctx) {
  const { h, cockpit, model } = ctx;
  let current = node;
  let level = 'full';
  let granted = false;
  let visible = false;
  let live = null;
  let liveTerminal = '';
  let focusWhenLive = false;
  let refocus = false; // it had the keys when it stopped drawing live
  let snapTimer = 0;
  let snapAt = 0;
  let snapping = false;
  let disposed = false;

  const screen = h('div', { class: 'cv-term-screen' });
  const snapshot = h('pre', { class: 'cv-term-snap', 'aria-label': 'The terminal\'s screen' });
  const coverText = h('span', { class: 'cv-cover-text' });
  const coverActions = h('span', { class: 'cv-cover-actions' });
  const cover = h('div', { class: 'cv-term-cover', hidden: true }, coverText, coverActions);
  const line = h('input', { type: 'text', spellcheck: 'false', autocomplete: 'off', placeholder: 'Send a line to the shell', 'aria-label': 'Send a line to the shell' });
  const sendForm = h('form', {
    class: 'cv-term-send', hidden: true,
    onsubmit: (event) => {
      event.preventDefault();
      const text = line.value;
      if (!text.trim()) return;
      line.value = '';
      model.send(current.id, { text, submit: true }).then(() => later(400), (error) => ctx.toast(error.message, 'error'));
    },
  }, h('span', { class: 'cv-prompt', 'aria-hidden': 'true', text: '❯' }), line);
  const box = h('div', { class: 'cv-term' }, screen, snapshot, cover, sendForm);

  const status = () => model.status.get(current.id) || { state: 'stopped' };
  const terminalID = () => current.runtime?.terminal || '';
  const serviceOn = () => cockpit.has('terminal');
  const harness = () => ctx.kinds.harness(current);
  const running = () => RUNNING.has(status().state) && !!terminalID();

  // ---------------------------------------------------------------- live

  function wantLive() {
    return granted && visible && level === 'full' && serviceOn() && running();
  }

  function sync() {
    if (disposed) return;
    const want = wantLive();
    if (live && (!want || liveTerminal !== terminalID())) unmount();
    if (want && !live) mount();
    box.dataset.live = live ? 'true' : '';
    snapshot.hidden = !!live;
    sendForm.hidden = serviceOn() || !running() || level === 'low';
    drawCover();
    scheduleSnapshot();
  }

  function mount() {
    liveTerminal = terminalID();
    const id = liveTerminal;
    live = cockpit.use('terminal').mount(screen, {
      id,
      scale: () => ctx.zoom(),
      onFocus: () => ctx.touch(),
      onReady: () => {
        const back = refocus && (!document.activeElement || document.activeElement === document.body);
        refocus = false;
        if ((focusWhenLive || back) && live) {
          focusWhenLive = false;
          live.focus();
        }
      },
      onExit: () => {
        // The shell ended: its status says so, and what it last showed stays.
        if (liveTerminal === id) later(0);
      },
    });
    screen.addEventListener('focusin', touched);
  }

  function unmount() {
    screen.removeEventListener('focusin', touched);
    // The element that has the keys goes with it, and no focusout says so.
    if (live && screen.contains(document.activeElement)) refocus = true;
    const was = live;
    live = null;
    liveTerminal = '';
    was?.dispose();
    screen.replaceChildren();
  }

  const touched = () => ctx.touch();

  // ---------------------------------------------------------------- snapshot

  // A snapshot is read when the node shows without a live terminal: every
  // couple of seconds while the shell works, less often while it waits.
  function scheduleSnapshot() {
    clearTimeout(snapTimer);
    snapTimer = 0;
    if (live || !visible || level === 'low' || !terminalID() || document.visibilityState !== 'visible') return;
    const busy = status().state === 'busy' || status().state === 'starting';
    const every = busy ? 2000 : 8000;
    const wait = Math.max(0, snapAt + every - Date.now());
    snapTimer = setTimeout(readSnapshot, snapAt ? wait : 0);
  }

  function later(ms) {
    clearTimeout(snapTimer);
    snapAt = 0;
    snapTimer = setTimeout(readSnapshot, ms);
  }

  async function readSnapshot() {
    snapTimer = 0;
    if (disposed || snapping || live || !terminalID()) return;
    snapping = true;
    snapAt = Date.now();
    try {
      const data = await model.read(current.id, 'screen');
      if (!disposed) snapshot.textContent = (data?.text || '').replace(/\s+$/, '');
    } catch {
      /* gone, or not running: the cover says so */
    } finally {
      snapping = false;
      if (!disposed) scheduleSnapshot();
    }
  }

  // ---------------------------------------------------------------- the cover

  function drawCover() {
    const st = status();
    const def = harness();
    const name = def?.title || 'The shell';
    let text = '';
    const actions = [];
    const restart = (label, resume) => h('button', {
      class: 'act strong', type: 'button',
      onclick: () => model.restart(current.id, resume).catch((error) => ctx.toast(error.message, 'error')),
    }, label);
    switch (st.state) {
      case 'starting':
        if (!live || !live.connected()) text = `${name} starts…`;
        break;
      case 'stopped':
        text = st.detail ? `Not running: ${st.detail}` : 'Not running';
        actions.push(restart('Start', false));
        if (def?.resume && current.runtime?.agent_session) actions.push(restart('Resume', true));
        break;
      case 'exited':
        text = st.detail || 'It ended';
        actions.push(restart('Restart', false));
        if (def?.resume && current.runtime?.agent_session) actions.push(restart('Resume', true));
        break;
      case 'error':
        text = st.detail || 'It failed';
        actions.push(restart('Restart', false));
        break;
      case 'paused':
        text = 'Waits for your approval';
        break;
      default:
        if (!terminalID()) text = 'No shell yet';
    }
    if (def && def.installed === false && st.state !== 'idle' && st.state !== 'busy') {
      text = `${def.program || def.title} is not installed here${text ? ` · ${text}` : ''}`;
    }
    cover.hidden = !text;
    box.dataset.cover = text ? st.state : '';
    if (coverText.textContent !== text) coverText.textContent = text;
    coverActions.replaceChildren(...actions);
  }

  // ---------------------------------------------------------------- the node's menu

  function menu() {
    const def = harness();
    const st = status();
    const items = [];
    if (running()) {
      items.push({ icon: '⌃', label: 'Send Ctrl-C', detail: 'Interrupt what runs', run: () => model.send(current.id, { keys: ['C-c'] }).catch((error) => ctx.toast(error.message, 'error')) });
      items.push({ icon: '⧉', label: 'Copy the screen', run: async () => {
        try {
          const data = await model.read(current.id, 'screen');
          cockpit.copy(data?.text || '', 'Screen copied');
        } catch (error) {
          ctx.toast(error.message, 'error');
        }
      } });
    }
    items.push({ icon: '↻', label: running() ? 'Restart' : 'Start', detail: running() ? 'End the shell and start it again' : '', run: () => model.restart(current.id, false).catch((error) => ctx.toast(error.message, 'error')) });
    if (def?.resume && current.runtime?.agent_session) {
      items.push({ icon: '↺', label: 'Resume', detail: `${def.title}'s last conversation`, run: () => model.restart(current.id, true).catch((error) => ctx.toast(error.message, 'error')) });
    }
    if (st.state !== 'stopped' && running()) {
      items.push({ icon: '■', label: 'Stop', detail: 'End the shell', run: () => model.stop(current.id).catch((error) => ctx.toast(error.message, 'error')) });
    }
    return items;
  }

  return {
    node: box,
    update(next) {
      current = next;
      sync();
    },
    status() {
      sync();
    },
    lod(nextLevel, nextGranted) {
      level = nextLevel;
      granted = !!nextGranted;
      sync();
    },
    shown() {
      visible = true;
      sync();
    },
    hidden() {
      visible = false;
      sync();
    },
    // focus enters the node: the keys go to the shell. A terminal that
    // does not draw live yet does once it is near enough.
    focus() {
      if (live) {
        live.focus();
        return true;
      }
      focusWhenLive = true;
      return false;
    },
    blur() {
      focusWhenLive = refocus = false;
      live?.blur();
    },
    resized() {
      live?.resize(true);
    },
    zoomed(zoom) {
      if (live && Math.abs(zoom - 1) < 0.001) live.resize(true);
    },
    contains: (target) => box.contains(target),
    live: () => !!live,
    menu,
    dispose() {
      disposed = true;
      clearTimeout(snapTimer);
      unmount();
    },
  };
}
