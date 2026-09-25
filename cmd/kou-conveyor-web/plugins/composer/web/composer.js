// composer: the prompt, in the layout's dock, with the banner of a run that
// did not finish and the activity of the run going on over it. It offers
// the slots composer.above (over the text: suggestions, images) and
// composer.row (beside the buttons: the model, the effort), and asks
// plugins through hooks:
//
//   composer.key     (event) → true when a plugin took the key
//   composer.submit  ({ raw, text, force, view }) → true when a plugin
//                    took what was written (a command, a queued message)
//
// Otherwise the prompt runs. It emits composer:input, composer:focus,
// composer:blur and composer:ready (input, form) once its elements exist,
// and provides the composer service: value(), set(text, { focus, end }),
// focus(), clear(), restore(text, images), input, form, draftKey(v),
// saveDraft(), forget(ws, id), submit({ force }).
export default function activate(cockpit) {
  const { h, fmt, prefs } = cockpit;
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
  const force = h('button', {
    class: 'force', id: 'force', type: 'button', hidden: true,
    title: 'Force it in: the agent reads it after the tool calls it is making, without waiting for the run to end',
    onclick: () => { submit({ force: true }); input.focus(); },
  }, h('span', { class: 'force-bolt', 'aria-hidden': 'true', text: '⚡' }), h('span', { text: 'Force' }), h('kbd', { text: '⌘' }), h('kbd', { text: '↵' }));
  const runLabel = h('span', { id: 'run-label', text: 'Run' });
  const runKey = h('kbd', { id: 'run-key', text: '↵' });
  const run = h('button', { class: 'run', id: 'run', type: 'submit', data: { mode: 'run' }, disabled: true }, runLabel, runKey);
  const form = h('form', { class: 'composer ticks', id: 'composer', autocomplete: 'off' },
    above, input, h('div', { class: 'composer-row' }, row, force, run));

  cockpit.ui.mount('dock', { id: 'resume', order: 10, node: resume });
  cockpit.ui.mount('dock', { id: 'activity', order: 20, node: activity });
  cockpit.ui.mount('dock', { id: 'composer', order: 40, node: form });
  cockpit.ui.slot('composer.above', above);
  cockpit.ui.slot('composer.row', row);

  // ---------------------------------------------------------------- text

  function autosize() {
    input.style.height = 'auto';
    input.style.height = `${Math.min(input.scrollHeight, Math.round(window.innerHeight * 0.4))}px`;
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
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      submit({ force: event.metaKey || event.ctrlKey });
    }
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
    run.dataset.mode = mode;
    run.disabled = mode === 'stop' ? v.run.phase === 'stopping' : mode === 'queue' ? false : (!typed || v.loading || v.external || v.gone || missing);
    runLabel.textContent = { run: 'Run', queue: 'Queue', stop: v.run?.phase === 'stopping' ? 'Stopping' : 'Stop' }[mode];
    runKey.textContent = mode === 'stop' ? 'Esc Esc' : '↵';
    run.title = mode === 'queue' ? 'Queue it: it runs as the next prompt when the agent finishes' : '';
    force.hidden = mode !== 'queue';
    input.placeholder = busy && queueing
      ? 'Message the agent. Enter queues it for when it finishes; ⌘Enter forces it in after its running tools.'
      : 'Describe the task. Enter runs it, Shift+Enter adds a line, / lists commands.';
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
