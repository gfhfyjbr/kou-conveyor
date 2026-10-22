// The body of a kou agent node: a session of the cockpit's own agent.
//
// Near enough, and the node the user had to do with last (lod.js), it is
// the whole chat — the cockpit's own, in a frame (../embed.js): the
// transcript with its thinking, the tools and their output, the composer
// with images, the model and the effort, the queue, Edit, Continue, the
// changes and the inspector, all as the stage has them. Clicking into it
// gives it the keys; Esc where Esc means nothing there, or Shift+Esc,
// gives them back to the board, and ⌘ and the wheel zoom the board.
//
// Otherwise it is a compact feed of the transcript (its tail read once,
// then what the canvas's events add) and a field to write to it. Enter
// sends (a run starts, or the message waits for the one running), ⌘Enter
// sends now, into the run, and Esc Esc stops it.
//
// The chat is there from when the node is made: before its first run, its
// session is a new one under the node's ID, whose first prompt — from the
// chat's composer as from the canvas — starts with the node's brief.
import { askFramesAgain, chatURL, framesAllowed, message, openable, MESSAGE } from '../embed.js';
import { CHATS, COARSE_CHATS } from '../lod.js';
import { streams } from '/kernel/events.js';

// The line a message from the canvas starts with (the engine's headerOf):
// [canvas] from «Title» (n_id):, or [canvas] reply from ….
const ORIGIN = /^\[canvas\] ((?:reply )?from) «([^»]*)»(?: \([^)\n]*\))?:\s*/;
const FAILED = new Set(['failed', 'error']);
const RUNNING = new Set(['queued', 'running']);
// How long a chat that does not say it is ready is waited for before it is
// shown anyway.
const READY_WITHIN = 12000;
// A chat its node no longer shows — the board panned away, zoomed out,
// another node given it — is kept a while, hidden, and shows again as it
// was if the node is back in time: a chat opened anew is the whole cockpit
// loading. No more are kept than show at once (lod.js). Where the page's
// streams are EventSources (one chat at a time), a chat holds connections
// the page has few of: one is kept only when its node went out of sight
// while no other chat shows or opens, and goes once one opens.
const KEEP_FOR = 10_000;
const KEPT = matchMedia('(pointer: coarse)').matches ? COARSE_CHATS : CHATS;
const kept = []; // the chats kept on the page, the oldest first: { timer, drop }
const holding = new Set(); // the nodes whose chats show or open on the page

export function createAgentBody(node, ctx) {
  const { h, fmt, model } = ctx;
  let current = node;
  let level = 'full';
  let visible = false;
  let granted = false;
  let loaded = false;
  let lastEscape = 0;
  let disposed = false;
  const rows = new Map(); // entry → its row

  const feed = h('div', { class: 'cv-feed', role: 'log', 'aria-label': 'What the agent did' });
  const EMPTY = 'Nothing yet: write to it, or wire something into it.';
  const empty = h('p', { class: 'cv-feed-empty', text: EMPTY });
  feed.append(empty);
  // A read of the transcript that failed is not "nothing yet": it is
  // said, and read again on Retry (or when the node shows next).
  const retryRead = h('button', {
    class: 'act', type: 'button',
    onclick: () => {
      empty.textContent = EMPTY;
      load();
    },
  }, 'Retry');
  const input = h('textarea', {
    class: 'cv-input', rows: '2', spellcheck: 'false',
    'aria-label': 'Message the agent', placeholder: 'Message it… ⏎ send · ⌘⏎ now · Esc Esc stop',
  });
  const working = h('span', { class: 'cv-working', hidden: true }, h('span', { class: 'meter', 'aria-hidden': 'true' }, h('i'), h('i'), h('i'), h('i'), h('i')), h('span', { class: 'cv-working-text' }));
  const model_ = h('button', { class: 'cv-chip', type: 'button', title: 'Its model: change it in the inspector', onclick: () => ctx.inspect() });
  const stop = h('button', { class: 'act', type: 'button', hidden: true, title: 'Stop the run (Esc Esc)', onclick: () => stopRun() }, 'Stop');
  const send = h('button', { class: 'act strong', type: 'button', title: 'Send (⏎); ⌘⏎ sends into the run now', onclick: () => submit(false) }, 'Send');
  const bar = h('div', { class: 'cv-agent-bar' }, working, model_, h('span', { class: 'cv-gap' }), stop, send);
  // The way to the whole chat, over the feed of a node that does not have it.
  const hint = h('button', {
    class: 'cv-chat-hint', type: 'button', hidden: true,
    title: 'Show the whole chat in this node: thinking, tools and their output, images, the queue, edits',
    onclick: (event) => {
      event.stopPropagation();
      wantFocus = true;
      ctx.entered?.();
    },
  }, h('span', { 'aria-hidden': 'true', text: '◆' }), 'Open the chat here');
  // Why a node with a session has its feed alone: the server does not let
  // the chat be framed (a build from before it did). It is asked again as
  // the server comes back.
  const note = h('p', {
    class: 'cv-chat-note', role: 'note', hidden: true,
    title: 'The server running does not let its pages be framed, so this node shows its feed. A build of the server with the chat in canvas nodes lets them: once it runs, the whole chat shows here, without reloading the page.',
  }, h('span', { 'aria-hidden': 'true', text: '◇' }), 'The chat here needs a newer server');
  const feedPane = h('div', { class: 'cv-agent-feed' }, feed, hint, note, h('div', { class: 'cv-agent-input' }, input, bar));
  const chatPane = h('div', { class: 'cv-chat', hidden: true });
  const chatState = h('p', { class: 'cv-chat-state', role: 'status', 'aria-live': 'polite' });
  const box = h('div', { class: 'cv-agent', data: { mode: 'feed' } }, feedPane, chatPane);

  const status = () => model.status.get(current.id) || { state: 'idle' };
  const sessionOf = () => current.runtime?.session || '';

  // ---------------------------------------------------------------- the chat

  let frame = null; // the iframe of the chat, while there is one
  let frameFor = ''; // the session it shows
  let keeping = null; // the chat is not shown, only kept (KEEP_FOR): { timer, drop }
  let readyTimer = 0;
  let gone = '';     // a session whose chat could not stay
  let before = null; // what had the keys when the chat came
  let wantFocus = false; // the user asked for the chat: it gets the keys once ready
  // Whether the server lets the chat be framed: null until it says.
  let framing = null;
  // Whether the node's session has a transcript (model.ran) is asked once:
  // a node has its session's ID from when it is made, the transcript once
  // a run of it began, and the stage shows only a session that has one.
  let asked = '';      // the session whose transcript was asked about
  let feeding = false; // a read of the feed, which says it too, is under way
  // Until the board says how much of the node shows (lod), it shows
  // neither its feed nor its chat: not the feed of a node whose chat comes.
  let decided = false;

  const wanted = () => granted && visible && level !== 'low' && !current.proposed && !!sessionOf() && gone !== sessionOf();
  // The node has its chat — shown, or opening — not merely kept.
  const held = () => !!frame && !keeping;
  const roomy = () => streams() === 'sockets' || streams() === 'h2';
  // The node's body is seen: in view, and more of it than its header.
  const seen = () => visible && level !== 'low';
  const chatting = () => box.dataset.mode === 'chat' || box.dataset.mode === 'chat-loading';

  // The node shows its feed ('feed'), its chat ('chat'), the chat opening
  // ('chat-loading') or, until decided, nothing (''). While the chat opens,
  // it says so; the feed stays only for one writing in its field, under the
  // chat, unseen till ready (canvas.css).
  function setMode(mode) {
    if (box.dataset.mode !== mode) box.dataset.mode = mode;
    const writing = mode === 'chat-loading' && document.activeElement === input;
    if (writing !== ('writing' in box.dataset)) {
      if (writing) box.dataset.writing = 'true';
      else delete box.dataset.writing;
    }
    feedPane.hidden = mode !== 'feed' && !writing;
    chatPane.hidden = mode !== 'chat' && mode !== 'chat-loading';
  }

  // showChat shows the node's chat: the one kept, if it is of its session,
  // as it was; otherwise one opened anew.
  function showChat() {
    const sid = sessionOf();
    if (frame && frameFor === sid) {
      if (!keeping) return;
      unkeep();
      holding.add(box);
      if (box.dataset.chat === 'ready') ready();
      else setMode('chat-loading');
      return;
    }
    dropChat();
    // Where streams are EventSources, the chats kept go first: the one that
    // opens needs their connections.
    if (!roomy()) for (const entry of [...kept]) entry.drop();
    holding.add(box);
    before = document.activeElement;
    frameFor = sid;
    frame = h('iframe', {
      class: 'cv-chat-frame', src: chatURL(model.ws, sid), title: `The chat of ${current.title || 'the agent'}`,
      // The cockpit's own page, of its own origin: it needs its scripts and
      // its storage, and opens nothing itself (what it would, the canvas does).
      referrerpolicy: 'same-origin', loading: 'eager',
    });
    chatState.textContent = 'Opening the chat…';
    chatPane.replaceChildren(frame, chatState);
    box.dataset.chat = 'loading';
    setMode('chat-loading');
    readyTimer = setTimeout(ready, READY_WITHIN);
  }

  // opening: the chat is wanted, and the server has not said yet whether it
  // may be framed. The node says the chat opens, not shows the feed a moment.
  function opening() {
    if (!frame) {
      chatState.textContent = 'Opening the chat…';
      if (!chatState.isConnected) chatPane.replaceChildren(chatState);
      box.dataset.chat = 'loading';
    }
    setMode('chat-loading');
  }

  function ready() {
    clearTimeout(readyTimer);
    if (!frame) return;
    box.dataset.chat = 'ready';
    // A chat kept shows as it is once its node wants it again: out of
    // sight, its node shows it ready already.
    if (keeping) {
      if (box.dataset.mode === 'chat-loading') setMode('chat');
      return;
    }
    // The feed's field had the keys, and what was written there went: the
    // chat's composer takes them.
    const writing = document.activeElement === input;
    setMode('chat');
    if (wantFocus || (writing && !input.value)) {
      wantFocus = false;
      // Not from under what is being written in the feed's field.
      if (!(writing && input.value)) focusChat();
    }
  }

  function focusChat() {
    frame.focus();
    tell('focus');
  }

  // release: the node shows its feed. Its chat is kept a while, hidden,
  // where the page has the room for it (KEEP_FOR); otherwise it goes. Out
  // of sight — the board panned away, zoomed out — the node stays as it
  // was: back in view it shows its chat, the one kept or one opening, not
  // its feed until the board has looked again (surface.js lodNow, once the
  // view stops).
  function release() {
    if (held()) {
      holding.delete(box);
      if (document.activeElement === frame) ctx.exit?.();
      if (gone !== frameFor && (roomy() || (!seen() && !holding.size))) keep();
      else dropChat();
    }
    if (!seen() && chatting()) {
      if (!frame) opening();
      return;
    }
    if (!frame && box.dataset.chat) {
      delete box.dataset.chat;
      chatPane.replaceChildren();
    }
    setMode('feed');
    if (seen()) load();
  }

  function keep() {
    const entry = {
      timer: setTimeout(() => entry.drop(), KEEP_FOR),
      drop: () => {
        const frozen = !seen() && chatting();
        dropChat();
        if (frozen) opening();
        drawHint();
      },
    };
    keeping = entry;
    kept.push(entry);
    while (kept.length > (roomy() ? KEPT : 1)) kept[0].drop();
  }

  function unkeep() {
    if (!keeping) return;
    clearTimeout(keeping.timer);
    const at = kept.indexOf(keeping);
    if (at >= 0) kept.splice(at, 1);
    keeping = null;
  }

  // dropChat lets the chat go, and its page with its frame.
  function dropChat() {
    unkeep();
    holding.delete(box);
    clearTimeout(readyTimer);
    if (frame) {
      if (document.activeElement === frame) ctx.exit?.();
      frame.remove();
    }
    frame = null;
    frameFor = '';
    delete box.dataset.chat;
    chatPane.replaceChildren();
  }

  function sync() {
    const sid = sessionOf();
    // Whether it has run is asked once a session, of a node in view; the
    // read of its feed, under way, says it as well.
    if (sid && !current.proposed && visible && level !== 'low' && !model.ran.has(sid) && asked !== sid) {
      asked = sid;
      if (!feeding) model.hasSession(current.id).catch(() => {});
    }
    if (framing === null && sid && !current.proposed) {
      framesAllowed().then((ok) => {
        framing = ok;
        if (!disposed) sync();
      });
      framing = undefined; // asked
    }
    if (wanted() && framing && chatURL(model.ws, sid)) showChat();
    else if (wanted() && framing == null) opening();
    else if (decided) release();
    drawHint();
  }

  function drawHint() {
    const could = !!sessionOf() && !current.proposed && level !== 'low' && gone !== sessionOf();
    hint.hidden = !could || !framing || held();
    note.hidden = !could || framing !== false;
  }

  // Light or dark, the chat as the page (../embed.js): the page says which
  // once the chat is ready, and each time it changes. What the user
  // switches in the chat, the page takes, and so every chat.
  const theme = () => document.documentElement.dataset.theme || '';
  const tell = (type, data = {}) => frame?.contentWindow?.postMessage({ ...data, [MESSAGE]: true, type }, location.origin);
  const offTheme = ctx.cockpit?.on?.('theme', () => tell('theme', { theme: theme() }));

  // The server came back — restarted, or took up a build of its own: a no
  // may be a yes now, and what it did not answer is asked again.
  const offServer = ctx.cockpit?.on?.('plugins-connected', () => {
    if (disposed) return;
    if (!model.ran.has(sessionOf())) asked = '';
    if (framing === false) {
      askFramesAgain();
      framing = null;
    }
    sync();
  });

  // The session has a transcript now (its first run began), or has none:
  // a chat that could not stay while it had none comes back with it.
  const offRan = model.on('ran', (sid) => {
    if (disposed || sid !== sessionOf()) return;
    if (gone === sid && model.ran.get(sid) === true) gone = '';
    sync();
  });

  function onMessage(event) {
    const data = frame && message(event, frame.contentWindow, location.origin);
    if (!data) return;
    switch (data.type) {
      case 'ready':
        tell('theme', { theme: theme() });
        ready();
        break;
      case 'theme':
        // Switched in the chat: the page takes it, and its chats with it.
        if ((data.theme === 'light' || data.theme === 'dark') && data.theme !== theme() && ctx.cockpit?.has?.('theme')) {
          ctx.cockpit.use('theme').set?.(data.theme);
        }
        break;
      case 'focus':
        ctx.entered?.();
        break;
      case 'unfocus':
        // The page in the frame took the keys as it came: they go back.
        if (document.activeElement === frame) {
          if (before?.isConnected && before !== frame) before.focus({ preventScroll: true });
          else ctx.exit?.();
        }
        break;
      case 'exit':
        ctx.exit?.();
        break;
      case 'zoom': {
        const r = frame.getBoundingClientRect();
        const k = frame.offsetWidth ? r.width / frame.offsetWidth : 1;
        const dy = Number(data.deltaY);
        if (Number.isFinite(dy)) ctx.zoomBy?.(dy, r.left + (Number(data.x) || 0) * k, r.top + (Number(data.y) || 0) * k);
        break;
      }
      case 'open':
        if (openable(data.hash)) location.hash = data.hash;
        break;
      case 'gone': {
        // The chat could not keep to its session. One with no transcript —
        // deleted, or none of whose runs began — fails nothing: the feed
        // shows what there is, and a run of it brings the chat back ('ran').
        // Another is said.
        const sid = frameFor;
        gone = sid;
        dropChat();
        release();
        drawHint();
        model.hasSession(current.id).then(() => {
          if (disposed || gone !== sid || model.ran.get(sid) === false) return;
          ctx.toast?.('The chat of this agent could not stay open here: its feed shows instead', 'error');
        }, () => {});
        break;
      }
    }
  }
  window.addEventListener('message', onMessage);

  // ---------------------------------------------------------------- the feed

  async function load() {
    if (loaded) return;
    loaded = true;
    feeding = true;
    const list = await model.feed(current.id);
    feeding = false;
    if (disposed) return;
    if (list.error) {
      loaded = false;
      if (!rows.size) {
        empty.replaceChildren('Could not load the transcript · ', retryRead);
        if (!empty.isConnected) feed.append(empty);
      }
      return;
    }
    if (empty.textContent !== EMPTY) empty.textContent = EMPTY;
    // The read did not say whether the session ran (it failed, or was
    // another's, from before): it is asked.
    if (!model.ran.has(sessionOf()) && asked === sessionOf()) {
      asked = '';
      sync();
    }
    // The list holds what the events brought meanwhile too, in order.
    for (const row of rows.values()) row.remove();
    rows.clear();
    for (const entry of list) put(entry);
    toEnd(true);
  }

  // put draws an entry, or draws it anew where it was.
  function put(entry) {
    if (!entry?.id) return;
    const row = rowOf(entry);
    const was = rows.get(entry.id);
    if (was) was.replaceWith(row);
    else feed.append(row);
    rows.set(entry.id, row);
    empty.remove();
    while (rows.size > 200) {
      const [first, el] = rows.entries().next().value;
      rows.delete(first);
      el.remove();
    }
  }

  function rowOf(entry) {
    switch (entry.kind) {
      case 'user': {
        const text = entry.text || '';
        const origin = ORIGIN.exec(text);
        return h('div', { class: 'cv-e cv-e-user', title: text },
          origin ? h('span', { class: 'cv-origin', text: `${origin[1]} ${origin[2]}` }) : h('span', { class: 'cv-origin you', text: 'you' }),
          h('span', { class: 'cv-e-text', text: (origin ? text.slice(origin[0].length) : text).replace(/\s+/g, ' ').trim() }));
      }
      case 'assistant': {
        const body = ctx.markdown(entry.text || '');
        return h('div', { class: 'cv-e cv-e-answer', data: { phase: entry.phase || null } }, body,
          entry.truncated ? h('small', { class: 'cv-e-more', text: 'Cut short: the chat shows it whole.' }) : null);
      }
      case 'reasoning':
        return h('div', { class: 'cv-e cv-e-thought', title: entry.text || '' }, h('span', { text: '· thinking' }));
      case 'tool': {
        const tool = entry.tool || {};
        const failed = FAILED.has(tool.state);
        const runningNow = RUNNING.has(tool.state);
        const took = tool.started && tool.finished ? fmt.duration(Date.parse(tool.finished) - Date.parse(tool.started)) : '';
        const mark = runningNow ? '…' : failed ? `✕${tool.exit_code ? ` ${tool.exit_code}` : ''}` : '✓';
        return h('div', { class: 'cv-e cv-e-tool', data: { state: failed ? 'failed' : runningNow ? 'running' : 'done' }, title: tool.summary || '' },
          h('span', { class: 'cv-e-name', text: `▸ ${tool.name || 'tool'}` }),
          tool.summary ? h('span', { class: 'cv-e-text', text: tool.summary.replace(/\s+/g, ' ') }) : null,
          h('span', { class: 'cv-e-mark', text: [mark, took].filter(Boolean).join(' ') }));
      }
      case 'error':
        return h('div', { class: 'cv-e cv-e-error', text: entry.text || 'It failed' });
      default:
        return h('div', { class: 'cv-e cv-e-notice', text: entry.text || entry.kind });
    }
  }

  const nearEnd = () => feed.scrollHeight - feed.scrollTop - feed.clientHeight < 48;
  function toEnd(force = false) {
    if (force || nearEnd()) feed.scrollTop = feed.scrollHeight;
  }

  const offAgent = model.on('agent', (event) => {
    if (event.node !== current.id || !event.entry) return;
    const end = nearEnd();
    put(event.entry);
    if (end) toEnd(true);
  });

  // ---------------------------------------------------------------- writing

  async function submit(now) {
    const text = input.value.trim();
    if (!text) return;
    input.value = '';
    try {
      await model.send(current.id, { text, deliver: now ? 'now' : '' });
    } catch (error) {
      input.value = text;
      ctx.toast(error.message, 'error');
    }
  }

  async function stopRun() {
    try {
      await model.stop(current.id);
    } catch (error) {
      ctx.toast(error.message, 'error');
    }
  }

  input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      event.stopPropagation();
      submit(event.metaKey || event.ctrlKey);
    } else if (event.key === 'Escape') {
      const now = Date.now();
      if (now - lastEscape < 600 && status().state === 'busy') {
        event.preventDefault();
        event.stopPropagation();
        lastEscape = 0;
        stopRun();
        return;
      }
      lastEscape = now;
    }
  });
  // A wheel over the feed scrolls it, not the canvas, when it can.
  feed.dataset.scroll = 'true';

  // ---------------------------------------------------------------- state

  function draw() {
    const st = status();
    const busy = st.state === 'busy' || st.state === 'starting';
    working.hidden = !busy;
    stop.hidden = !busy;
    const text = st.activity || (st.state === 'starting' ? 'starting' : 'working');
    const label = working.querySelector('.cv-working-text');
    if (label.textContent !== text) label.textContent = text;
    const config = current.config || {};
    const name = config.model ? ctx.modelTitle(config.model) : 'default model';
    const chip = [name, config.sandbox === 'worktree' ? 'worktree' : ''].filter(Boolean).join(' · ');
    if (model_.textContent !== chip) model_.textContent = chip;
    input.disabled = !!current.proposed;
  }

  function lastAnswer() {
    const list = model.feeds.get(current.id) || [];
    for (let i = list.length - 1; i >= 0; i--) if (list[i].kind === 'assistant' && list[i].text) return list[i].text;
    return '';
  }

  function menu() {
    const sessionID = sessionOf();
    return [
      sessionID && framing && !held() && level !== 'low' && {
        icon: '◆', label: 'Open the chat here', detail: 'Thinking, tools and their output, images, the queue, edits — in this node',
        run: () => { wantFocus = true; ctx.entered?.(); },
      },
      // A session none of whose runs began is not there to open.
      sessionID && model.ran.get(sessionID) !== false && { icon: '↗', label: 'Open session', detail: 'The whole session, on the stage', run: () => ctx.openSession(sessionID) },
      status().state === 'busy' && { icon: '■', label: 'Stop the run', hint: 'Esc Esc', run: stopRun },
      { icon: '⧉', label: 'Copy the last answer', disabled: !lastAnswer(), run: () => ctx.cockpit.copy(lastAnswer(), 'Answer copied') },
    ].filter(Boolean);
  }

  draw();
  setMode('');
  drawHint();
  return {
    node: box,
    update(next) {
      current = next;
      draw();
      sync();
    },
    status: draw,
    runtime: () => sync(),
    lod(nextLevel, live = false) {
      level = nextLevel;
      granted = !!live;
      decided = true;
      if (level !== 'low' && visible && !held()) load();
      sync();
    },
    shown() {
      visible = true;
      if (level !== 'low') load();
      sync();
    },
    hidden() {
      visible = false;
      sync();
    },
    focus() {
      if (held() && box.dataset.chat === 'ready') {
        focusChat();
        return true;
      }
      if (wanted() || (sessionOf() && level !== 'low')) wantFocus = true;
      input.focus();
      return true;
    },
    // Esc in the board's own fields: the chat takes them back.
    blur() {
      if (frame && document.activeElement === frame) frame.blur();
    },
    contains: (target) => box.contains(target),
    menu,
    dispose() {
      disposed = true;
      offAgent();
      offRan();
      offServer?.();
      offTheme?.();
      window.removeEventListener('message', onMessage);
      unkeep();
      holding.delete(box);
      clearTimeout(readyTimer);
      frame?.remove();
      frame = null;
    },
  };
}
