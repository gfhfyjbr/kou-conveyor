// The body of a kou agent node: a session of the cockpit's own agent.
//
// The node is the whole chat — the cockpit's own, in a frame (../embed.js):
// the transcript with its thinking, the tools and their output, the
// composer with images, the model and the effort, the queue, Edit,
// Continue, the changes and the inspector, all as the stage has them.
// Clicking into it gives it the keys; Esc where Esc means nothing there,
// or Shift+Esc, gives them back to the board, and ⌘ and the wheel zoom the
// board.
//
// Each chat is a whole cockpit page, so the board keeps them in hand: the
// nodes in view at a zoom where they can be read have theirs, as many as
// the page has room for (lod.js); they open a few at a time (BOOTING), and
// each reads only the end of its transcript, the rest on demand
// (../embed.js). A node without its chat shows no transcript of its own:
// it says why — its turn to come, a proposal not approved yet, a server
// that does not let it be framed — and offers the way to it.
//
// The chat is there from when the node is made: before its first run, its
// session is a new one under the node's ID, whose first prompt — from the
// chat's composer as from the canvas — starts with the node's brief.
import { askFramesAgain, chatURL, framesAllowed, message, openable, MESSAGE } from '../embed.js';
import { streams } from '/kernel/events.js';

// How long a chat that does not say it is ready is waited for before it is
// shown anyway.
const READY_WITHIN = 12000;
// BOOTING is how many chats open at once. Each is a whole cockpit page
// booting on the board's own thread: many at once, and the board stutters
// until they are done. The others wait their turn, and open as one has
// booted ('booted') — not once it read its session, which is the server's
// work, seconds for a long one, and goes on side by side.
const BOOTING = 2;
const line = { open: new Set(), waiting: [] }; // the turns to open a chat: taken, and waited for
// A chat its node no longer shows — the board panned away, zoomed out,
// another node given it — is kept a while, hidden, and shows again as it
// was if the node is back in time: a chat opened anew is the whole cockpit
// loading. A chat kept holds as much as one shown, so only a few are
// (KEPT). Where the page's streams are EventSources (one chat at a time),
// a chat holds connections the page has few of: one is kept only when its
// node went out of sight while no other chat shows or opens, and goes once
// one opens.
const KEEP_FOR = 10_000;
const KEPT = matchMedia('(pointer: coarse)').matches ? 1 : 4;
const kept = []; // the chats kept on the page, the oldest first: { timer, drop }
const holding = new Set(); // the nodes whose chats show, open or wait to

// turn asks for a turn to open a chat: start runs once it comes — at once
// while fewer than BOOTING open. What it returns gives the turn back (the
// chat is ready, or went), or leaves the line; once is enough.
function turn(start) {
  const t = { start };
  line.waiting.push(t);
  nextTurn();
  return () => {
    if (line.open.delete(t)) {
      nextTurn();
      return;
    }
    const at = line.waiting.indexOf(t);
    if (at >= 0) line.waiting.splice(at, 1);
  };
}

function nextTurn() {
  while (line.open.size < BOOTING && line.waiting.length) {
    const t = line.waiting.shift();
    line.open.add(t);
    try {
      t.start();
    } catch (error) {
      // A chat that could not start opening does not hold its turn.
      line.open.delete(t);
      console.error(error);
    }
  }
}

export function createAgentBody(node, ctx) {
  const { h, model } = ctx;
  let current = node;
  let level = 'full';
  let visible = false;
  let granted = false;
  let disposed = false;

  // Why the node has no chat, and the way to it (drawOff).
  const offText = h('p', { class: 'cv-chat-off-text' });
  const offActs = h('div', { class: 'cv-chat-off-acts' });
  const offPane = h('div', { class: 'cv-chat-off', hidden: true }, offText, offActs);
  const chatPane = h('div', { class: 'cv-chat', hidden: true });
  const chatState = h('p', { class: 'cv-chat-state', role: 'status', 'aria-live': 'polite' });
  // The node takes the keys itself while it has no chat to give them to;
  // Esc gives them back to the board.
  const box = h('div', { class: 'cv-agent', tabindex: '-1', data: { mode: '' } }, offPane, chatPane);
  box.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape' || event.target !== box || event.isComposing) return;
    event.preventDefault();
    event.stopPropagation();
    ctx.exit?.();
  });

  const status = () => model.status.get(current.id) || { state: 'idle' };
  const sessionOf = () => current.runtime?.session || '';

  // ---------------------------------------------------------------- the chat

  let frame = null; // the iframe of the chat, while there is one
  let frameFor = ''; // the session it shows
  let waitingFor = ''; // the session whose chat waits for its turn to open
  let giveTurn = null; // gives the turn to open back, while it is had or waited for
  let keeping = null; // the chat is not shown, only kept (KEEP_FOR): { timer, drop }
  let readyTimer = 0;
  let gone = '';     // a session whose chat could not stay
  let before = null; // what had the keys when the chat came
  let wantFocus = false; // the user asked for the chat: it gets the keys once ready
  // Whether the server lets the chat be framed: null until it says.
  let framing = null;
  // Until the board says how much of the node shows (lod), it shows
  // neither its chat nor why it has none: not that of a node whose chat
  // comes.
  let decided = false;

  const wanted = () => granted && visible && level !== 'low' && !current.proposed && !!sessionOf() && gone !== sessionOf();
  // The node has its chat — shown, or opening — not merely kept.
  const held = () => !!frame && !keeping;
  const roomy = () => streams() === 'sockets' || streams() === 'h2';
  // The node's body is seen: in view, and more of it than its header.
  const seen = () => visible && level !== 'low';
  const chatting = () => box.dataset.mode === 'chat' || box.dataset.mode === 'chat-loading';

  // The node shows its chat ('chat'), the chat opening ('chat-loading'),
  // why it has none and the way to it ('off') or, until decided, nothing
  // ('').
  function setMode(mode) {
    if (box.dataset.mode !== mode) box.dataset.mode = mode;
    offPane.hidden = mode !== 'off';
    chatPane.hidden = mode !== 'chat' && mode !== 'chat-loading';
  }

  // showChat shows the node's chat: the one kept, if it is of its session,
  // as it was; otherwise one opened anew, once its turn comes.
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
    if (waitingFor === sid) return;
    dropChat();
    // Where streams are EventSources, the chats kept go first: the one that
    // opens needs their connections.
    if (!roomy()) for (const entry of [...kept]) entry.drop();
    holding.add(box);
    waitingFor = sid;
    chatState.textContent = 'Opening the chat…';
    chatPane.replaceChildren(chatState);
    box.dataset.chat = 'loading';
    setMode('chat-loading');
    giveTurn = turn(() => openChat(sid));
  }

  // openChat opens the chat, its turn come: the cockpit's page, in a frame.
  function openChat(sid) {
    waitingFor = '';
    if (disposed) {
      passTurn();
      return;
    }
    before = document.activeElement;
    frameFor = sid;
    frame = h('iframe', {
      class: 'cv-chat-frame', src: chatURL(model.ws, sid), title: `The chat of ${current.title || 'the agent'}`,
      // The cockpit's own page, of its own origin: it needs its scripts and
      // its storage, and opens nothing itself (what it would, the canvas does).
      referrerpolicy: 'same-origin', loading: 'eager',
    });
    chatPane.replaceChildren(frame, chatState);
    readyTimer = setTimeout(ready, READY_WITHIN);
  }

  function passTurn() {
    const give = giveTurn;
    giveTurn = null;
    give?.();
  }

  // opening: the chat is wanted, and the server has not said yet whether it
  // may be framed. The node says the chat opens.
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
    // Open, it lets the next chat that waits open.
    passTurn();
    box.dataset.chat = 'ready';
    // A chat kept shows as it is once its node wants it again: out of
    // sight, its node shows it ready already.
    if (keeping) {
      if (box.dataset.mode === 'chat-loading') setMode('chat');
      return;
    }
    setMode('chat');
    // Asked for, or given the keys while it opened, the chat takes them.
    if (wantFocus || document.activeElement === box) {
      wantFocus = false;
      focusChat();
    }
  }

  function focusChat() {
    frame.focus();
    tell('focus');
  }

  // release: the node has no chat. Its chat is kept a while, hidden, where
  // the page has the room for it (KEEP_FOR); otherwise it goes. Out of
  // sight — the board panned away, zoomed out — the node stays as it was:
  // back in view it shows its chat, the one kept or one opening, until the
  // board has looked again (surface.js lodNow, once the view stops).
  function release() {
    if (waitingFor) {
      waitingFor = '';
      holding.delete(box);
      passTurn();
    }
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
    setMode('off');
  }

  function keep() {
    const entry = {
      timer: setTimeout(() => entry.drop(), KEEP_FOR),
      drop: () => {
        const frozen = !seen() && chatting();
        dropChat();
        if (frozen) opening();
        drawOff();
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

  // dropChat lets the chat go, and its page with its frame, or its turn.
  function dropChat() {
    unkeep();
    holding.delete(box);
    clearTimeout(readyTimer);
    waitingFor = '';
    passTurn();
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
    drawOff();
  }

  // drawOff says why the node has no chat, and offers the way to it.
  let offDrawn = '';
  function drawOff() {
    const sid = sessionOf();
    // Whether the session has a transcript (model.ran) is not asked ahead:
    // a chat reads its session itself, and each read of one is the whole
    // file on the server — seconds for a long one, and a board's worth at
    // once holds its chats up. Not known, the session is offered, and
    // opening it asks (canvas.js openAgentSession); a chat gone asks
    // ('gone').
    const ran = sid ? model.ran.get(sid) : undefined;
    // A session none of whose runs began is not there to open.
    const openSession = sid && ran !== false ? ['Open session', () => ctx.openSession?.(sid)] : null;
    let text;
    let acts = [];
    if (current.proposed) {
      text = 'An agent proposed it: approve it, and its chat opens here.';
    } else if (!sid) {
      text = 'Its chat opens here once it has a session.';
    } else if (framing === false) {
      // A build of the server from before the chat in canvas nodes. It is
      // asked again as the server comes back (plugins-connected).
      text = 'The server running does not let its pages be framed: the chat opens here once a newer build of it runs.';
      acts = [openSession];
    } else if (gone === sid) {
      text = ran === false ? 'Its session is not there: deleted, or none of its runs began yet.' : 'Its chat could not stay open here.';
      acts = [['Retry', () => {
        gone = '';
        wantFocus = true;
        ctx.entered?.();
        sync();
      }, true], openSession];
    } else {
      text = roomy()
        ? 'So many chats are open on the canvas that this one waits: it opens when you use it.'
        : 'Over this connection the page holds one chat at a time: this one opens when you use it.';
      acts = [['Open the chat here', () => {
        wantFocus = true;
        ctx.entered?.();
      }, true]];
    }
    acts = acts.filter(Boolean);
    const drawn = JSON.stringify([sid, text, acts.map(([label]) => label)]);
    if (drawn === offDrawn) return;
    offDrawn = drawn;
    offText.textContent = text;
    offActs.replaceChildren(...acts.map(([label, run, strong]) => h('button', {
      class: strong ? 'act strong' : 'act', type: 'button',
      onclick: (event) => {
        event.stopPropagation();
        run();
      },
    }, label)));
  }

  // Light or dark, the chat as the page (../embed.js): the page says which
  // once the chat is ready, and each time it changes. What the user
  // switches in the chat, the page takes, and so every chat.
  const theme = () => document.documentElement.dataset.theme || '';
  const tell = (type, data = {}) => frame?.contentWindow?.postMessage({ ...data, [MESSAGE]: true, type }, location.origin);
  const offTheme = ctx.cockpit?.on?.('theme', () => tell('theme', { theme: theme() }));

  // The server came back — restarted, or took up a build of its own: its
  // no to framing may be a yes now, and is asked again.
  const offServer = ctx.cockpit?.on?.('plugins-connected', () => {
    if (disposed) return;
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
      case 'booted':
        // Its page booted, and reads its session: the next chat may open.
        passTurn();
        break;
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
        // The chat could not keep to its session: the node says so, and
        // offers to try again. One with no transcript — deleted, or none of
        // whose runs began — fails nothing: a run of it brings the chat back
        // ('ran'). Another is said.
        const sid = frameFor;
        gone = sid;
        dropChat();
        release();
        drawOff();
        model.hasSession(current.id).then(() => {
          if (disposed || gone !== sid) return;
          drawOff();
          if (model.ran.get(sid) === false) return;
          ctx.toast?.('The chat of this agent could not stay open here', 'error');
        }, () => {});
        break;
      }
    }
  }
  window.addEventListener('message', onMessage);

  async function stopRun() {
    try {
      await model.stop(current.id);
    } catch (error) {
      ctx.toast(error.message, 'error');
    }
  }

  function menu() {
    const sessionID = sessionOf();
    return [
      sessionID && framing && !held() && level !== 'low' && !current.proposed && {
        icon: '◆', label: 'Open the chat here', detail: 'Thinking, tools and their output, images, the queue, edits — in this node',
        run: () => { wantFocus = true; ctx.entered?.(); },
      },
      sessionID && model.ran.get(sessionID) !== false && { icon: '↗', label: 'Open session', detail: 'The whole session, on the stage', run: () => ctx.openSession(sessionID) },
      status().state === 'busy' && { icon: '■', label: 'Stop the run', run: stopRun },
    ].filter(Boolean);
  }

  setMode('');
  drawOff();
  return {
    node: box,
    update(next) {
      current = next;
      sync();
    },
    runtime: () => sync(),
    lod(nextLevel, live = false) {
      level = nextLevel;
      granted = !!live;
      decided = true;
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
    focus() {
      if (held() && box.dataset.chat === 'ready') {
        focusChat();
        return true;
      }
      // Its chat comes (it is the node used last now), and takes the keys.
      if (sessionOf() && level !== 'low' && !current.proposed) wantFocus = true;
      box.focus({ preventScroll: true });
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
      offRan();
      offServer?.();
      offTheme?.();
      window.removeEventListener('message', onMessage);
      unkeep();
      holding.delete(box);
      clearTimeout(readyTimer);
      waitingFor = '';
      passTurn();
      frame?.remove();
      frame = null;
    },
  };
}
