// composer: the prompt, in the layout's dock, with the banner of a run that
// did not finish, the activity of the run going on or, while it compacts the
// context, the summary it writes over it instead, and the session's figures
// by it: the cache's hits, the context left, the speed, the turns — in a
// tray under it, on a line there or in a chip in its row, folded under it
// or not, as its section of the settings says (Session figures). Its row,
// under the text, ends with the buttons that send it: icons alone. It
// offers the slots composer.above (over the text: suggestions, images) and
// composer.row (beside the buttons: the model, the effort), and asks plugins
// through hooks:
//
//   composer.key     (event) → true when a plugin took the key
//   composer.submit  ({ raw, text, force, view }) → true when a plugin
//                    took what was written (a command, a queued message)
//
// Otherwise the prompt runs. On a touch screen — a phone, or a tablet
// without a mouse — Enter adds a line, as the keyboard's return key does
// anywhere else, and the button runs the prompt (⌘Enter still does, on a
// keyboard attached). It emits composer:input, composer:focus,
// composer:blur and composer:ready (input, form) once its elements exist,
// and provides the composer service: value(), set(text, { focus, end }),
// focus(), clear(), restore(text, images), input, form, draftKey(v),
// saveDraft(), forget(ws, id), submit({ force }).
// The buttons' icons: Run, Stop, Queue (a list and a plus) and Force; and
// the arrow of the figures folded under the prompt.
const ICONS = {
  run: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5 2.75v10.5L13.25 8z" fill="currentColor"/></svg>',
  stop: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.75 3.75h8.5v8.5h-8.5z" fill="currentColor"/></svg>',
  queue: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 3.75h10M2 7.75h10M2 11.75h5.5M12 9v6M9 12h6" fill="none" stroke="currentColor" stroke-width="1.5"/></svg>',
  force: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M9.25 1.25 3.5 9h4l-1.25 5.75L12.5 7h-4z" fill="currentColor"/></svg>',
  fold: '<svg viewBox="0 0 10 6" aria-hidden="true"><path d="M1 1l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5"/></svg>',
};

// The looks of the session's figures, as Settings names them, and the
// figures, in their order.
const LOOKS = {
  tray: { name: 'Tray', title: 'A tray under the prompt, each figure in a cell of its own' },
  line: { name: 'Line', title: 'A quiet line under the prompt, with no frame' },
  chip: { name: 'Chip', title: 'A button in the prompt\'s row; its card has them all' },
  off: { name: 'Off', title: 'None: the inspector\'s Tokens has them all' },
};
const FIGURES = ['cache', 'context', 'speed', 'turns'];
// What the chip says by its first figure shown: the context left first.
const FACE = { context: '', cache: 'cache', speed: 'tok/s', turns: 'turns' };
const FACE_ORDER = ['context', 'cache', 'speed', 'turns'];

export default function activate(cockpit) {
  const { h, svg, fmt, prefs } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();
  const hot = cockpit.hot.data;

  // ---------------------------------------------------------------- the dock

  const resumeText = h('span', { class: 'resume-text', id: 'resume-text' });
  const resumeEdit = h('button', { class: 'act', id: 'resume-edit', type: 'button', title: 'Edit the last prompt and run it again from there (Esc Esc)', hidden: true, onclick: () => service('edit')?.editLast?.() }, 'Edit prompt');
  const resumeRun = h('button', { class: 'act strong', id: 'resume-run', type: 'button', onclick: onResume }, 'Continue');
  const resumeDismiss = h('button', {
    class: 'icon small', id: 'resume-dismiss', type: 'button', 'aria-label': 'Dismiss', title: 'Dismiss',
    onclick: () => {
      const v = view();
      // The offer to restart goes with its run; the offer to continue stays
      // dismissed until the next run ends.
      if (resume.dataset.kind === 'outdated') { if (v.run) v.run.outdatedDismissed = true; }
      else v.resumeDismissed = true;
      cockpit.render();
    },
  }, '×');
  const resume = h('div', { class: 'resume', id: 'resume', hidden: true }, h('span', { class: 'resume-mark', 'aria-hidden': 'true' }), resumeText, resumeEdit, resumeRun, resumeDismiss);

  const activityText = h('span', { id: 'activity-text', text: 'Working' });
  const activityClock = h('time', { id: 'activity-clock' });
  const activity = h('div', { class: 'activity', id: 'activity', hidden: true },
    h('span', { class: 'meter', 'aria-hidden': 'true' }, Array.from({ length: 8 }, () => h('i'))),
    activityText, activityClock,
    h('span', { class: 'activity-hint' }, h('kbd', { text: 'Esc' }), h('kbd', { text: 'Esc' }), ' stop'));

  // While the run compacts the context, the summary it writes streams over
  // the prompt: the end of it, in a few lines, and three bars until it starts.
  const summaryPart = h('span', { class: 'compacting-part' });
  const summaryBars = h('div', { class: 'compacting-bars', 'aria-hidden': 'true' }, h('i'), h('i'), h('i'));
  const summaryText = h('div', { class: 'compacting-text', hidden: true });
  const summaryBody = h('div', { class: 'compacting-body' }, summaryBars, summaryText);
  const compacting = h('section', { class: 'compacting', id: 'compacting', hidden: true, 'aria-label': 'The summary of the conversation, as it is written' },
    h('div', { class: 'compacting-head' }, h('span', { class: 'compacting-mark', 'aria-hidden': 'true' }), h('span', { text: 'Compacting…' }), summaryPart),
    summaryBody);

  const above = h('div', { class: 'slot-contents' });
  const input = h('textarea', {
    id: 'prompt', rows: '1', spellcheck: 'true', 'aria-label': 'Prompt', 'aria-autocomplete': 'list', 'aria-controls': 'commands', 'aria-expanded': 'false',
    placeholder: 'Describe the task. Enter runs it, Shift+Enter adds a line, / lists commands.',
  });
  const row = h('div', { class: 'slot-contents' });
  // What sends the prompt ends the row: one button — Run, Stop while the
  // agent works — and, once something is written while it works, two: Force
  // and Queue. Icons alone: their names and keys are in their labels and
  // titles.
  const force = h('button', {
    class: 'send', id: 'force', type: 'button', data: { mode: 'force' }, hidden: true, 'aria-label': 'Force it in',
    title: 'Force it in: the agent reads it after the tool calls it is making, without waiting for the run to end (⌘↵)',
    onclick: () => { submit({ force: true }); input.focus(); },
  }, svg(ICONS.force));
  const run = h('button', { class: 'send', id: 'run', type: 'submit', data: { mode: 'run' }, 'aria-label': 'Run', disabled: true }, svg(ICONS.run));
  const form = h('form', { class: 'composer ticks', id: 'composer', autocomplete: 'off' },
    above, input, h('div', { class: 'composer-row' }, row, h('div', { class: 'composer-send' }, force, run)));

  // By the prompt, the session's figures at a glance (the inspector's Tokens
  // has them all): the share of the input the cache served, the context
  // left, how fast the model writes and its turns. The shares have a meter.
  // A long name has a short one for a narrow tray. Where they go is the
  // settings' (the figures' look, below): a tray under the prompt, a line
  // there, or the card of a chip in its row.
  const figure = (key, name, metered, short = '') => {
    const value = h('b', { class: 'stat-v' });
    const unit = h('span', { class: 'stat-u' });
    const meter = metered ? h('span', { class: 'stat-meter', 'aria-hidden': 'true' }, h('i')) : null;
    const label = short
      ? h('span', { class: 'stat-k' }, h('span', { class: 'stat-long', text: name }), h('span', { class: 'stat-short', text: short }))
      : h('span', { class: 'stat-k', text: name });
    const node = h('span', { class: 'stat', data: { stat: key } }, label, meter, value, unit);
    return { name, node, value, unit, meter, spec: null };
  };
  const figures = {
    cache: figure('cache', 'Cache', true),
    context: figure('context', 'Context', true, 'Ctx'),
    speed: figure('tps', 'TPS', false),
    turns: figure('turns', 'Turns', false),
  };
  // The tray, or the line, in a fold that holds it under the prompt when it
  // is compact; then an arrow, which slides out from under the prompt's foot
  // as the pointer comes to the prompt, brings it out and folds it again.
  const stats = h('div', { class: 'stats', id: 'stats', role: 'group', 'aria-label': 'Session figures' });
  const tab = h('button', {
    class: 'stats-tab', id: 'stats-tab', type: 'button', 'aria-label': 'Session figures', 'aria-controls': 'stats', 'aria-expanded': 'false', hidden: true,
    onclick: () => pin(!look.open),
  }, svg(ICONS.fold));
  const tray = h('div', { class: 'stats-dock', id: 'stats-dock', hidden: true }, h('div', { class: 'stats-fold' }, stats), tab);
  // The chip: the first figure shown — the context left, on a dial, unless
  // it is not shown — and over it a card of them all.
  const dial = h('span', { class: 'stats-dial', 'aria-hidden': 'true' }, h('i'));
  const faceValue = h('b', { class: 'stats-face-v' });
  const faceUnit = h('span', { class: 'stats-face-u' });
  const face = h('button', {
    class: 'stats-face', id: 'stats-face', type: 'button', 'aria-label': 'Session figures', 'aria-controls': 'stats-card', 'aria-expanded': 'false',
    onclick: () => openCard(chip.dataset.open !== 'true'),
  }, dial, faceValue, faceUnit);
  const cardGrid = h('div', { class: 'stats-grid' });
  const cardNote = h('p', { class: 'stats-note' });
  const card = h('div', { class: 'stats-card', id: 'stats-card', role: 'group', 'aria-label': 'Session figures' }, cardGrid, cardNote);
  const chip = h('div', { class: 'stats-chip', id: 'stats-chip', hidden: true, data: { open: 'false' } }, face, card);
  // The figures' look (the settings', kept in this browser), and the
  // settings' sections drawn, which follow it.
  let look = kept();
  const synced = new Set();

  cockpit.ui.mount('dock', { id: 'resume', order: 10, node: resume });
  cockpit.ui.mount('dock', { id: 'activity', order: 20, node: activity });
  cockpit.ui.mount('dock', { id: 'compacting', order: 25, node: compacting });
  cockpit.ui.mount('dock', { id: 'composer', order: 40, node: form });
  cockpit.ui.mount('dock', { id: 'stats', order: 50, node: tray });
  cockpit.ui.slot('composer.above', above);
  cockpit.ui.slot('composer.row', row);
  cockpit.ui.mount('composer.row', { id: 'figures', order: 30, node: chip });

  // A touch screen: a phone, or a tablet without a mouse. Its keyboard has
  // no Shift+Enter, nor keys to hint at.
  const touch = window.matchMedia('(hover: none) and (pointer: coarse)');
  cockpit.listen(touch, 'change', () => cockpit.render());

  // ---------------------------------------------------------------- text

  // shown is the height of the page in sight: on a phone, what the keyboard
  // leaves of it (pinch zoom aside).
  function shown() {
    const viewport = window.visualViewport;
    return Math.min(window.innerHeight, viewport ? viewport.height * viewport.scale : Infinity);
  }

  function autosize() {
    input.style.height = 'auto';
    input.style.height = `${Math.min(input.scrollHeight, Math.round(shown() * 0.4))}px`;
  }

  function set(text, { focus = false, end = false } = {}) {
    input.value = text;
    autosize();
    cockpit.render();
    cockpit.emit('composer:input', input.value);
    if (focus || end) input.focus();
    if (end) input.setSelectionRange(input.value.length, input.value.length);
  }

  // Drafts: what was written in a session stays with it.
  const draftKey = (v) => `draft.${v.ws || 'default'}.${v.fresh ? 'new' : v.id}`;
  let draftTimer = 0;

  function saveDraft(v = view()) {
    if (!v) return;
    const text = input.value;
    prefs.set(draftKey(v), text.trim() ? text : null);
    service('images')?.saveDraft?.(draftKey(v));
  }

  function loadDraft(v) {
    set(prefs.get(draftKey(v), '') || '');
    service('images')?.loadDraft?.(draftKey(v));
  }

  // clear empties the composer and its draft, once what it held went.
  function clear({ images = true } = {}) {
    const v = view();
    clearTimeout(draftTimer);
    set('');
    if (v) prefs.set(draftKey(v), null);
    if (images && v) service('images')?.dropDraft?.(draftKey(v));
  }

  // restore puts back what did not go, unless something new was written.
  function restore(text, images) {
    if (input.value.trim()) return;
    set(text);
    if (images) service('images')?.set?.(images);
  }

  function forget(ws, id) {
    prefs.set(`draft.${ws || 'default'}.${id}`, null);
  }

  // ---------------------------------------------------------------- running

  function submit({ force: forced = false } = {}) {
    const v = view();
    if (!v) return;
    const raw = input.value;
    const text = raw.trim();
    if (!text) return;
    // A command, or a message for the agent at work, is a plugin's.
    if (cockpit.hooks.first('composer.submit', { raw, text, force: forced, view: v })) return;
    session.submit(text, { fromComposer: true });
  }

  cockpit.listen(form, 'submit', (event) => {
    event.preventDefault();
    const v = view();
    if (v?.run && (!input.value.trim() || v.edit)) session.stop();
    else submit();
  });
  cockpit.listen(input, 'keydown', (event) => {
    if (cockpit.hooks.first('composer.key', event)) return;
    if (event.key !== 'Enter' || event.shiftKey || event.isComposing) return;
    const modified = event.metaKey || event.ctrlKey;
    // A touch screen's Enter adds a line: the button runs the prompt.
    if (touch.matches && !modified) return;
    event.preventDefault();
    submit({ force: modified });
  });
  cockpit.listen(input, 'input', () => {
    autosize();
    cockpit.render();
    cockpit.emit('composer:input', input.value);
    clearTimeout(draftTimer);
    const v = view();
    draftTimer = setTimeout(() => { if (view() === v) saveDraft(v); }, 300);
  });
  cockpit.listen(input, 'focus', () => cockpit.emit('composer:focus'));
  cockpit.listen(input, 'blur', () => cockpit.emit('composer:blur'));
  cockpit.listen(window, 'resize', autosize);
  // A phone's keyboard, as it opens and closes, changes what is in sight.
  if (window.visualViewport) cockpit.listen(window.visualViewport, 'resize', autosize);
  // The stage changes width with the window, and as the rail or a panel
  // opens, closes or is resized: the text wraps anew.
  cockpit.on('layout:resize', autosize);
  cockpit.listen(window, 'pagehide', () => saveDraft());
  cockpit.onDispose(() => clearTimeout(draftTimer));

  cockpit.on('session:leave', (v) => {
    clearTimeout(draftTimer);
    saveDraft(v);
  });
  cockpit.on('session:view', (v, { reload } = {}) => { if (!reload) loadDraft(v); });

  // ---------------------------------------------------------------- the summary

  // SUMMARY_KEEP is how much of the end of a summary its block holds: more
  // than its lines show, however wide they are.
  const SUMMARY_KEEP = 2000;
  let summaryDrawn = null;

  // drawSummary shows what streamed of a compaction's summary (the session's
  // view.compaction): the end of its latest piece — the thinking before it,
  // or its text — without the tags around the review and the summary nor
  // blank lines, which its few lines have no room for, with the part it is
  // in. A runner from before the review was named streams it as <analysis>.
  function drawSummary(compaction) {
    compacting.hidden = !compaction;
    const piece = compaction ? [...compaction.pieces.values()].at(-1) || null : null;
    if (!compaction || piece === summaryDrawn) return;
    summaryDrawn = piece;
    const raw = piece?.text || '';
    const thinking = piece?.kind === 'reasoning';
    summaryPart.textContent = thinking ? 'thinking' : raw.includes('<summary>') ? 'summary' : /<(?:review|analysis)>/.test(raw) ? 'review' : '';
    const text = raw
      .replace(/<\/?(?:review|analysis|summary)>/g, '')
      .replace(/<\/?[a-z]*$/, '') // a tag still coming
      .replace(/\n\s*\n/g, '\n')
      .trimStart()
      .slice(-SUMMARY_KEEP);
    compacting.dataset.kind = thinking ? 'reasoning' : 'assistant';
    summaryBars.hidden = !!text;
    summaryText.hidden = !text;
    summaryText.textContent = text;
    // The lines that went out of sight at the top fade out.
    summaryBody.dataset.over = text && summaryText.offsetHeight > summaryBody.clientHeight ? 'true' : 'false';
  }

  // ---------------------------------------------------------------- the figures

  // The conversation is compacted before its window is full: unless
  // KOU_CONVEYOR_AUTO_COMPACT says otherwise, 33k short of it (a quarter of
  // a small one), and at 200k at most, 390k for a Claude model on the
  // Messages API, whose IDs carry no vendor (agentrunner, compactionLimits).
  // The context left is what is left until then.
  const compactsAt = (size, model) =>
    Math.min(size - Math.min(33_000, size / 4), /^claude-/i.test(model) ? 390_000 : 200_000);
  // Tokens per second: output tokens over the time their turns took, as the
  // inspector's Tokens counts them.
  const perSecond = (tokens, ms) => (tokens > 0 && ms > 0 ? (tokens * 1000) / ms : NaN);
  const rate = (n) => (n >= 100 ? String(Math.round(n)) : n.toFixed(1));

  // The changes below touch only what differs: the figures are drawn on
  // every render, once a second while the agent works. A share is a
  // property, which composer.css draws, in whole cells or not.
  const fill = (el, name, share) => {
    const value = Math.max(0, Math.min(1, share ?? 0)).toFixed(3);
    if (el.style.getPropertyValue(name) !== value) el.style.setProperty(name, value);
  };
  const toned = (el, tone) => {
    if ((el.dataset.tone || '') === tone) return;
    if (tone) el.dataset.tone = tone;
    else delete el.dataset.tone;
  };
  const say = (el, value) => { if (el.textContent !== value) el.textContent = value; };

  // put draws a figure. A share fills its meter; with none, the meter goes.
  function put(f, { value, unit = '', share = null, tone = '', title }) {
    f.spec = { value, unit, share, tone, title };
    say(f.value, value);
    say(f.unit, unit);
    if (f.node.title !== title) f.node.title = title;
    toned(f.node, tone);
    if (!f.meter) return;
    f.meter.hidden = share == null;
    fill(f.meter, '--fill', share);
  }

  function drawStats(v) {
    const usage = v?.usage;
    const chipped = look.style === 'chip';
    const shown = look.style !== 'off' && look.hide.length < FIGURES.length
      && !!(usage?.turns || usage?.input) && !v.loading && !v.gone;
    tray.hidden = !shown || chipped;
    chip.hidden = !shown || !chipped;
    if (chip.hidden) openCard(false);
    if (!shown) return;

    const hit = usage.input > 0 ? usage.cached / usage.input : NaN;
    put(figures.cache, Number.isFinite(hit)
      ? { value: `${Math.round(hit * 100)}%`, share: hit, title: `Cache hits: ${fmt.tokens(usage.cached)} of the ${fmt.tokens(usage.input)} input tokens came from the cache` }
      : { value: '—', title: 'Cache hits: the share of the input tokens the cache served' });

    // The window is that of the model the next prompt runs with, when the
    // list of models knows it; else the figure is what the context holds.
    const models = service('models');
    const model = models?.next?.(v) || models?.default?.() || '';
    const known = models?.find?.(model);
    const size = known?.context || 0;
    const used = usage.context || 0;
    // The chip's card says under its figures whose window it is.
    let note;
    if (size > 0) {
      const at = compactsAt(size, model);
      const left = Math.max(0, at - used);
      const label = models.contextLabel?.(size) || fmt.tokens(size);
      put(figures.context, {
        value: fmt.tokens(left), unit: 'left', share: left / at,
        tone: left <= at * 0.1 ? 'err' : left <= at * 0.25 ? 'warn' : '',
        title: `Context left before the conversation is compacted: the latest request held ${fmt.tokens(used)} of about ${fmt.tokens(at)}, in the ${label} window of ${known.name || model}`,
      });
      note = `${known.name || model} · ${label} window · compacted at about ${fmt.tokens(at)}`;
    } else {
      put(figures.context, {
        value: fmt.tokens(used), unit: 'used',
        title: `Context: the latest request held ${fmt.tokens(used)} tokens; the window of ${known?.name || model || 'the model'} is not known here`,
      });
      note = `${known?.name || model || 'The model'}: its window is not known here`;
    }

    // The latest turn's speed, or the session's while none is timed.
    const last = perSecond(usage.last_output, usage.last_ms);
    const average = perSecond(usage.timed_output, usage.timed_ms);
    const speed = Number.isFinite(last) ? last : average;
    put(figures.speed, {
      value: Number.isFinite(speed) ? rate(speed) : '—',
      title: !Number.isFinite(last) ? 'Output tokens per second, once a turn reports its output tokens'
        : `Output tokens per second: the latest turn wrote ${fmt.tokens(usage.last_output)} tokens in ${fmt.duration(usage.last_ms)}`
          + (Number.isFinite(average) && usage.timed_ms !== usage.last_ms ? `; ${rate(average)} on average over the session` : ''),
    });

    put(figures.turns, { value: usage.turns ? String(usage.turns) : '—', title: 'Turns: the requests to the model in this session' });

    // Folded, the arrow under the prompt takes the colour of a context
    // running out.
    toned(tray, look.hide.includes('context') ? '' : figures.context.spec.tone);
    if (chipped) drawFace(note);
  }

  // drawFace draws the chip: the first figure shown, the context left
  // unless it is not, on a dial when it is a share.
  function drawFace(note) {
    const key = FACE_ORDER.find((k) => !look.hide.includes(k)) || 'context';
    const spec = figures[key].spec || { value: '—', unit: '', share: null, tone: '' };
    const unit = FACE[key] || spec.unit;
    say(faceValue, spec.value);
    say(faceUnit, unit);
    dial.hidden = spec.share == null;
    fill(dial, '--stats-dial', spec.share);
    toned(face, spec.tone);
    const label = `Session figures: ${spec.value} ${unit}`.trim();
    if (face.getAttribute('aria-label') !== label) face.setAttribute('aria-label', label);
    say(cardNote, look.hide.includes('context') ? '' : note);
  }

  // ---------------------------------------------------------------- the figures' look

  // kept reads the look this browser keeps: by default the tray, not
  // folded — once it is, its arrow comes out at the pointer — with every
  // figure.
  function kept() {
    const style = prefs.get('figures.style', 'tray');
    const hide = prefs.get('figures.hide', []);
    return {
      style: typeof style === 'string' && Object.hasOwn(LOOKS, style) ? style : 'tray',
      compact: prefs.get('figures.compact', false) === true,
      hover: prefs.get('figures.hover', true) !== false,
      open: prefs.get('figures.open', false) === true,
      hide: Array.isArray(hide) ? FIGURES.filter((k) => hide.includes(k)) : [],
    };
  }

  // choose changes the look and keeps it, but what is as by default.
  function choose(change) {
    look = { ...look, ...change };
    prefs.set('figures.style', look.style === 'tray' ? null : look.style);
    prefs.set('figures.compact', look.compact ? true : null);
    prefs.set('figures.hover', look.hover ? null : false);
    prefs.set('figures.open', look.open ? true : null);
    prefs.set('figures.hide', look.hide.length ? look.hide : null);
    applyLook();
  }

  // applyLook puts the figures shown where the look has them — the tray or
  // the line, or the chip's card — in their order, and the others out of
  // the page, so that only the figures shown have rules between them.
  function applyLook() {
    const chipped = look.style === 'chip';
    const home = chipped ? cardGrid : stats;
    const nodes = FIGURES.filter((k) => !look.hide.includes(k)).map((k) => figures[k].node);
    if (!(home.childNodes.length === nodes.length && nodes.every((node, i) => home.childNodes[i] === node))) home.replaceChildren(...nodes);
    for (const k of look.hide) figures[k].node.remove();
    (chipped ? stats : cardGrid).replaceChildren();
    tray.dataset.style = look.style === 'line' ? 'line' : 'tray';
    tray.dataset.compact = String(look.compact);
    tray.dataset.hover = String(look.hover);
    tray.dataset.open = String(look.compact && look.open);
    tab.hidden = !look.compact;
    tab.setAttribute('aria-expanded', String(look.open));
    tab.title = look.open ? 'Fold the figures under the prompt' : 'Show the session\'s figures';
    if (!chipped) openCard(false);
    for (const section of synced) {
      if (!section.node.isConnected) synced.delete(section);
      else cockpit.safely(() => section.sync());
    }
    cockpit.render();
  }

  // pin brings the folded figures out and keeps them out, or folds them
  // again.
  function pin(open) {
    choose({ open });
  }

  // openCard opens the chip's card, or closes it: closed under the pointer,
  // it stays closed until the pointer leaves.
  function openCard(open) {
    open = !!open;
    if ((chip.dataset.open === 'true') === open) return;
    chip.dataset.open = String(open);
    face.setAttribute('aria-expanded', String(open));
    if (open) place();
    else if (chip.matches(':hover')) chip.dataset.hush = '';
  }
  // place keeps the card over the composer: it opens to the right of the
  // chip's edge, unless that is out of the composer.
  function place() {
    const box = form.getBoundingClientRect();
    const from = chip.getBoundingClientRect().left;
    const dx = Math.max(Math.min(0, box.right - 8 - (from + card.offsetWidth)), box.left + 8 - from);
    card.style.setProperty('--dx', `${Math.round(dx)}px`);
  }
  cockpit.listen(chip, 'pointerenter', () => place());
  cockpit.listen(chip, 'pointerleave', () => { delete chip.dataset.hush; });
  cockpit.on('layout:resize', () => { if (chip.dataset.open === 'true') place(); });
  // A press elsewhere closes the card; so does Escape, as other popovers.
  cockpit.listen(document, 'pointerdown', (event) => {
    if (chip.dataset.open === 'true' && !chip.contains(event.target)) openCard(false);
  }, true);
  cockpit.contribute('overlay', { id: 'stats-card', order: 90, isOpen: () => chip.dataset.open === 'true', close: () => openCard(false) });
  // Another tab chose another look: so does this one.
  cockpit.listen(window, 'storage', (event) => {
    if (event.key !== null && !String(event.key).startsWith('kou-conveyor.figures.')) return;
    look = kept();
    applyLook();
  });

  // ---------------------------------------------------------------- settings

  let drawings = 0;
  cockpit.contribute('settings.section', {
    id: 'figures', title: 'Session figures', order: 12,
    render() {
      const group = `figures-${++drawings}`;
      const looks = h('div', { class: 'look', role: 'radiogroup', 'aria-label': 'Look' },
        Object.entries(LOOKS).map(([key, l]) => h('label', { title: l.title },
          h('input', { type: 'radio', name: `${group}-look`, value: key, onchange: () => choose({ style: key }) }),
          h('span', null, h('i', { class: 'stats-mark', data: { look: key } }), l.name))));
      const foldBox = h('input', { type: 'checkbox', onchange: (event) => choose({ compact: event.currentTarget.checked, open: false }) });
      const hoverBox = h('input', { type: 'checkbox', onchange: (event) => choose({ hover: event.currentTarget.checked }) });
      const foldRow = h('label', { class: 'set-toggle' }, foldBox, 'Fold them under the prompt');
      const hoverRow = h('label', { class: 'set-toggle' }, hoverBox, 'Show the arrow only while the pointer is on the prompt');
      const which = h('div', { class: 'look', role: 'group', 'aria-label': 'Figures shown' },
        FIGURES.map((key) => h('label', null,
          h('input', {
            type: 'checkbox', value: key,
            onchange: (event) => choose({ hide: FIGURES.filter((k) => (k === key ? !event.currentTarget.checked : look.hide.includes(k))) }),
          }),
          h('span', null, figures[key].name))));
      const node = h('div', null,
        h('div', { class: 'set-row' }, h('span', { class: 'label', text: 'Look' }), looks,
          h('p', { class: 'set-note', text: 'The cache\'s hits, the context left before the conversation is compacted, the tokens per second of the latest turn and the turns, by the prompt: in a tray under it, as they were first; on a quiet line there; in a chip in its row, whose card has them all; or nowhere — the inspector\'s Tokens has them all.' })),
        h('div', { class: 'set-row' }, foldRow, hoverRow,
          h('p', { class: 'set-note', text: 'Folded, the tray or the line hides under the prompt. As the pointer comes to the prompt, an arrow slides out from under it: a click on it brings the figures out, another folds them again, and out, they have the arrow under them the same way. Without the second switch, or on a touch screen, the arrow stays out.' })),
        h('div', { class: 'set-row' }, h('span', { class: 'label', text: 'Figures' }), which,
          h('p', { class: 'set-note', text: 'One at least. All of this is kept in this browser.' })));
      function sync() {
        for (const input of looks.querySelectorAll('input')) input.checked = input.value === look.style;
        const folds = look.style === 'tray' || look.style === 'line';
        foldBox.checked = look.compact;
        hoverBox.checked = look.hover;
        foldBox.disabled = !folds;
        hoverBox.disabled = !folds || !look.compact;
        foldRow.dataset.off = String(foldBox.disabled);
        hoverRow.dataset.off = String(hoverBox.disabled);
        const left = FIGURES.length - look.hide.length;
        for (const input of which.querySelectorAll('input')) {
          input.checked = !look.hide.includes(input.value);
          // The last figure shown stays: Off hides them all.
          input.disabled = look.style === 'off' || (input.checked && left <= 1);
          input.parentElement.dataset.off = String(look.style === 'off');
        }
      }
      sync();
      synced.add({ node, sync });
      return node;
    },
  });

  // ---------------------------------------------------------------- the banner

  function onResume() {
    switch (resume.dataset.kind) {
      case 'missing':
        session.removeWorkspace(session.state.ws);
        return;
      case 'gone': {
        // Carry the unsent prompt over to a fresh session.
        const text = input.value;
        session.newSession();
        if (text.trim()) set(text);
        return;
      }
      case 'outdated':
        session.restart?.();
        return;
      default:
        session.submit(session.CONTINUE);
    }
  }

  function render() {
    const v = view();
    if (!v) {
      tray.hidden = true;
      chip.hidden = true;
      return;
    }
    const w = session.currentWorkspace();
    const missing = !!w?.missing;
    // A run whose runner was built anew since it started goes on with the
    // build it started with: a restart goes on from where it got to with the
    // new one (session.restart).
    const outdated = !!v.run?.outdated && v.run.phase !== 'stopping' && !v.run.outdatedDismissed && !v.run.compact && !!session.restart;
    const banner = missing ? 'missing' : v.gone ? 'gone'
      : v.interrupted && !v.run && !v.external && !v.loading && !v.resumeDismissed && !v.edit ? 'resume'
      : outdated ? 'outdated' : '';
    resume.hidden = !banner;
    resume.dataset.kind = banner;
    resumeText.textContent = {
      missing: `The folder ${w?.display || ''} no longer exists. Runs are off until it is back.`,
      gone: 'This session was deleted in another window. Its prompt can go to a new session.',
      resume: 'The last run did not finish. Continue it, or edit its prompt.',
      outdated: 'The runner was built anew since this run started, and the run goes on with the old build. Restart it to go on from here with the new one.',
    }[banner] || '';
    resumeRun.textContent = { missing: 'Remove from list', gone: 'New session', outdated: 'Restart' }[banner] || 'Continue';
    resumeRun.title = banner === 'outdated' ? 'Stop the run and start it again at once, with the new runner, from where it got to' : '';
    resumeEdit.hidden = banner !== 'resume' || !session.lastPrompt(v) || !cockpit.has('edit');
    resumeDismiss.hidden = banner !== 'resume' && banner !== 'outdated';

    const busy = !!v.run;
    const typed = !!input.value.trim();
    // While the agent works, a prompt is queued (or forced in); without one,
    // the button stops the run.
    const queueing = cockpit.has('queue');
    const mode = busy ? (typed && !v.edit && queueing ? 'queue' : 'stop') : 'run';
    const stopping = v.run?.phase === 'stopping';
    if (run.dataset.mode !== mode) {
      run.dataset.mode = mode;
      run.replaceChildren(svg(ICONS[mode]));
    }
    run.disabled = mode === 'stop' ? stopping : mode === 'queue' ? false : (!typed || v.loading || v.external || v.gone || missing);
    const label = { run: 'Run', queue: 'Queue', stop: stopping ? 'Stopping' : 'Stop' }[mode];
    if (run.getAttribute('aria-label') !== label) run.setAttribute('aria-label', label);
    run.title = {
      run: 'Run (↵)',
      queue: 'Queue it: it runs as the next prompt when the agent finishes (↵)',
      stop: stopping ? 'Stopping' : 'Stop the agent (Esc Esc)',
    }[mode];
    force.hidden = mode !== 'queue';
    const files = cockpit.has('files');
    // A touch screen's names no keys, and fits a line of a phone's (from
    // 360px wide).
    const placeholder = touch.matches
      ? busy && queueing ? 'Message the agent at work' : 'Describe the task · / commands'
      : busy && queueing
        ? 'Message the agent. Enter queues it for when it finishes; ⌘Enter forces it in after its running tools.'
        : `Describe the task. Enter runs it, Shift+Enter adds a line, / lists commands${files ? ', $ links a file' : ''}.`;
    if (input.placeholder !== placeholder) {
      input.placeholder = placeholder;
      autosize(); // a placeholder of another length takes other lines
    }
    // While the run compacts the context, the compaction's block says so; the
    // line comes back for what else happens meanwhile: a request to the model
    // tried again, a stop.
    activity.hidden = !busy || (!!v.compaction && v.run.activity === 'Compacting context');
    activityText.textContent = v.run?.activity || 'Working';
    activityClock.textContent = v.run ? fmt.timer(Date.now() - v.run.started) : '';
    drawSummary(busy ? v.compaction : null);
    form.dataset.busy = busy ? 'true' : 'false';
    drawStats(v);
  }
  cockpit.on('render', render);

  // ---------------------------------------------------------------- what others use

  cockpit.provide('composer', {
    input, form,
    value: () => input.value,
    set, clear, restore, forget, draftKey, saveDraft: () => saveDraft(), autosize,
    focus: () => input.focus(),
    submit,
    // Retry and Reuse put a prompt back into the composer, its images too.
    reuse(text, entry) {
      set(text);
      const v = view();
      if (entry?.images?.length) service('images')?.load?.(entry.images, (n) => service('images').imageURL(v, entry, n));
      input.focus();
    },
  });

  cockpit.contribute('timeline.action', {
    id: 'retry', kinds: ['user'], order: 5, label: 'Retry', title: 'Put this prompt back into the composer',
    shown: (entry, ctx) => entry.state === 'undelivered' && !ctx.editable,
    run: (entry) => cockpit.use('composer').reuse(entry.text, entry),
  });
  cockpit.contribute('timeline.action', {
    id: 'reuse', kinds: ['user'], order: 30, label: 'Reuse', title: 'Copy this prompt into the composer, to send it again at the end',
    shown: (entry) => entry.state !== 'undelivered',
    run: (entry) => cockpit.use('composer').reuse(entry.text, entry),
  });

  cockpit.keys.register({ key: 'c', views: ['sessions'], run: () => input.focus() });
  cockpit.contribute('help.keys', { keys: ['↵'], text: 'Run the prompt · while the agent works, queue it for when it finishes', order: 10 });
  cockpit.contribute('help.keys', { keys: ['⇧', '↵'], text: 'New line', order: 40 });
  cockpit.contribute('help.keys', { keys: ['C'], text: 'Focus the composer', order: 200 });

  // Loaded anew, it takes up what the version before had written.
  if (hot.text != null) {
    set(hot.text);
    hot.text = null;
  } else if (view()) {
    loadDraft(view());
  }
  cockpit.onDispose(() => { hot.text = input.value; });
  cockpit.emit('composer:ready', input, form);
  applyLook();
}
