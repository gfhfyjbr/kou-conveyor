// The foreman's field, at the foot of the board: what is asked there goes
// to the canvas's foreman — a kou agent that builds and keeps the canvas,
// made by the engine with the first question (a canvas not saved yet is
// saved then). What it answers shows in its node, and for a while in a
// bubble over the field. ⏎ asks, ⇧⏎ starts a new line, Esc gives the keys
// back to the board.
const MAX_HEIGHT = 160;
const BUBBLE_FOR = 20000;

export function createForeman(env) {
  const { h, cockpit } = env;
  const input = h('textarea', {
    class: 'cv-foreman-input', rows: '1', spellcheck: 'true', autocomplete: 'off',
    placeholder: 'Ask the foreman to build or change the canvas…', 'aria-label': 'Ask the foreman',
  });
  const state = h('span', { class: 'cv-foreman-state', 'aria-live': 'polite' });
  const stop = h('button', { class: 'act', type: 'button', hidden: true, title: 'Stop the foreman\'s run', onclick: () => stopRun() }, 'Stop');
  const send = h('button', { class: 'cv-foreman-send', type: 'submit', title: 'Ask (⏎)', 'aria-label': 'Ask the foreman' }, '⏎');
  const field = h('form', { class: 'cv-foreman-field', onsubmit: (event) => { event.preventDefault(); ask(); } },
    h('span', { class: 'cv-foreman-glyph', 'aria-hidden': 'true', text: '◈' }), input, state, stop, send);
  const bubbleText = h('div', { class: 'cv-bubble-text' });
  const bubble = h('div', { class: 'cv-bubble', hidden: true, role: 'status' },
    h('div', { class: 'cv-bubble-head' }, h('span', { class: 'label', text: '◈ Foreman' }),
      h('button', { class: 'act', type: 'button', title: 'Show its node, with all it said', onclick: () => openNode() }, 'Open'),
      h('button', { class: 'icon small', type: 'button', 'aria-label': 'Close', onclick: () => hideBubble() }, '×')),
    bubbleText);
  const element = h('div', { class: 'cv-foreman' }, bubble, field);

  let sending = false;
  let bubbleTimer = 0;

  const model = () => env.model();
  const foremanNode = () => model()?.doc.nodes.find((n) => n.kind === 'agent' && n.preset === 'foreman') || null;

  function autosize() {
    input.style.height = 'auto';
    input.style.height = `${Math.min(MAX_HEIGHT, input.scrollHeight)}px`;
    input.style.overflowY = input.scrollHeight > MAX_HEIGHT ? 'auto' : 'hidden';
  }

  async function ask() {
    const m = model();
    const text = input.value.trim();
    if (!m || !text || sending) return;
    if (m.readOnly) {
      cockpit.toast('This canvas is read only here.', 'error');
      return;
    }
    sending = true;
    draw();
    try {
      const result = await m.foreman(text);
      if (model() === m) {
        input.value = '';
        autosize();
        env.sent?.(result);
      }
    } catch (error) {
      cockpit.toast(`The foreman did not get it: ${error.message}`, 'error');
    } finally {
      sending = false;
      draw();
    }
  }

  function stopRun() {
    const n = foremanNode();
    if (n) model()?.stop(n.id).catch((error) => cockpit.toast(error.message, 'error'));
  }

  function openNode() {
    const n = foremanNode();
    hideBubble();
    if (n) env.reveal?.(n.id);
  }

  function hideBubble() {
    clearTimeout(bubbleTimer);
    bubble.hidden = true;
  }

  // answered shows what the foreman put out, for a while.
  function answered(event) {
    const n = foremanNode();
    if (!n || event?.node !== n.id || event.log) return;
    const text = (event.preview || event.title || '').trim();
    if (!text) return;
    bubbleText.replaceChildren(env.markdown(text));
    bubble.hidden = false;
    clearTimeout(bubbleTimer);
    bubbleTimer = setTimeout(hideBubble, BUBBLE_FOR);
  }

  // draw says what the foreman does now.
  function draw() {
    const m = model();
    const n = foremanNode();
    const st = n ? m.status.get(n.id) : null;
    const busy = st?.state === 'busy' || st?.state === 'starting';
    element.hidden = !m || m.readOnly || m.deleted;
    let text = '';
    if (sending) text = 'sending…';
    else if (busy) text = st.activity || 'working…';
    else if (st?.state === 'error') text = st.detail || 'it failed';
    const waiting = n ? m.pendingFor(n.id) : 0;
    if (waiting && !sending) text = [text, `${waiting} waiting`].filter(Boolean).join(' · ');
    if (state.textContent !== text) state.textContent = text;
    state.title = text;
    element.dataset.state = sending ? 'sending' : busy ? 'busy' : st?.state || '';
    stop.hidden = !busy;
    send.disabled = sending;
    input.placeholder = n ? 'Ask the foreman…' : 'Ask the foreman to build or change the canvas…';
  }

  input.addEventListener('input', autosize);
  input.addEventListener('keydown', (event) => {
    if (event.isComposing) return;
    if (event.key === 'Enter' && !event.shiftKey && !event.altKey) {
      event.preventDefault();
      ask();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      input.blur();
      env.done?.();
    }
  });

  return {
    element,
    draw,
    answered,
    value: () => input.value,
    set(text) {
      input.value = String(text || '');
      autosize();
    },
    focus() {
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    },
    contains: (el) => element.contains(el),
    // height is how much of the board's foot the field covers.
    height: () => (element.hidden ? 0 : element.getBoundingClientRect().height + 16),
    dispose() {
      clearTimeout(bubbleTimer);
      element.remove();
    },
  };
}
