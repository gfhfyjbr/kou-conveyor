// composer: the prompt, in the layout's dock, with the banner of a run that
// did not finish and the activity of the run going on over it. Its row, under
// the text, ends with the buttons that send it: icons alone. It offers the
// slots composer.above (over the text: suggestions, images) and composer.row
// (beside the buttons: the model, the effort), and asks plugins through
// hooks:
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
// The buttons' icons: Run, Stop, Queue (a list and a plus) and Force.
const ICONS = {
  run: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5 2.75v10.5L13.25 8z" fill="currentColor"/></svg>',
  stop: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.75 3.75h8.5v8.5h-8.5z" fill="currentColor"/></svg>',
  queue: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 3.75h10M2 7.75h10M2 11.75h5.5M12 9v6M9 12h6" fill="none" stroke="currentColor" stroke-width="1.5"/></svg>',
  force: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M9.25 1.25 3.5 9h4l-1.25 5.75L12.5 7h-4z" fill="currentColor"/></svg>',
};

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
    onclick: () => { view().resumeDismissed = true; cockpit.render(); },
  }, '×');
  const resume = h('div', { class: 'resume', id: 'resume', hidden: true }, h('span', { class: 'resume-mark', 'aria-hidden': 'true' }), resumeText, resumeEdit, resumeRun, resumeDismiss);

  const activityText = h('span', { id: 'activity-text', text: 'Working' });
  const activityClock = h('time', { id: 'activity-clock' });
  const activity = h('div', { class: 'activity', id: 'activity', hidden: true },
    h('span', { class: 'meter', 'aria-hidden': 'true' }, Array.from({ length: 8 }, () => h('i'))),
    activityText, activityClock,
    h('span', { class: 'activity-hint' }, h('kbd', { text: 'Esc' }), h('kbd', { text: 'Esc' }), ' stop'));

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

  cockpit.ui.mount('dock', { id: 'resume', order: 10, node: resume });
  cockpit.ui.mount('dock', { id: 'activity', order: 20, node: activity });
  cockpit.ui.mount('dock', { id: 'composer', order: 40, node: form });
  cockpit.ui.slot('composer.above', above);
  cockpit.ui.slot('composer.row', row);

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
      default:
        session.submit(session.CONTINUE);
    }
  }

  function render() {
    const v = view();
    if (!v) return;
    const w = session.currentWorkspace();
    const missing = !!w?.missing;
    const banner = missing ? 'missing' : v.gone ? 'gone'
      : v.interrupted && !v.run && !v.external && !v.loading && !v.resumeDismissed && !v.edit ? 'resume' : '';
    resume.hidden = !banner;
    resume.dataset.kind = banner;
    resumeText.textContent = {
      missing: `The folder ${w?.display || ''} no longer exists. Runs are off until it is back.`,
      gone: 'This session was deleted in another window. Its prompt can go to a new session.',
      resume: 'The last run did not finish. Continue it, or edit its prompt.',
    }[banner] || '';
    resumeRun.textContent = { missing: 'Remove from list', gone: 'New session' }[banner] || 'Continue';
    resumeEdit.hidden = banner !== 'resume' || !session.lastPrompt(v) || !cockpit.has('edit');
    resumeDismiss.hidden = banner !== 'resume';

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
    activity.hidden = !busy;
    activityText.textContent = v.run?.activity || 'Working';
    activityClock.textContent = v.run ? fmt.timer(Date.now() - v.run.started) : '';
    form.dataset.busy = busy ? 'true' : 'false';
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
  cockpit.render();
}
