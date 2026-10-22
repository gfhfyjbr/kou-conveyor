// voice: dictation in the composer. The microphone button before the send
// buttons listens while it is held and writes what is said at the caret as
// it is said: push to talk. A click starts it and another click stops it.
// While it listens Enter stops it and runs the prompt, Esc takes back what
// it wrote, and typing stops it, keeping what it wrote.
//
// The microphone streams to the cockpit (the WebSocket /voice/socket), which
// streams it on to the speech-to-text provider chosen in the settings:
// Soniox, Deepgram, ElevenLabs, OpenAI, Gemini, Groq or Yandex. The words
// come back as they are said from the live ones, the last few revised until
// the provider is sure of them, and from the others once the take stops;
// letting go asks the provider to be sure of all of them. The cockpit tells
// it of the browser's languages, and of the names of the workspace's folders
// and files, which it then spells as the workspace does. Browsers give the
// microphone only to a secure page: the cockpit over https, or on localhost.
//
// The providers' keys are the cockpit's, the same in every workspace, and
// never reach the page (GET, PUT and DELETE /api/voice): variables of its
// environment, or the keys pasted in the dialog the button opens when there
// is none, which the cockpit keeps beside its settings. /voice opens it.

const ICON = '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="5.25" y="1.25" width="5.5" height="8.5" rx="2.75" fill="currentColor"/><path d="M3 7.5a5 5 0 0 0 10 0M8 12.5v2.25" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>';

const HOLD = 350; // ms: a press held longer is push to talk, a shorter one a click
const TAIL = 150; // ms listened to after letting go: the end of the last word
const RATE = 16000; // Hz of the audio sent
const WAIT = 75_000; // ms the last words may take once the take stops; the cockpit gives up first

export default function activate(cockpit) {
  const { h, svg, toast } = cockpit;
  const composer = cockpit.use('composer');
  const input = composer.input;
  const touch = window.matchMedia('(hover: none) and (pointer: coarse)');

  const button = h('button', {
    class: 'voice', type: 'button', 'aria-label': 'Dictate', 'aria-pressed': 'false',
    title: 'Dictate: hold to talk, or click, talk and click again. Enter runs it, Esc takes it back. Right-click: the settings.',
  }, svg(ICON));
  cockpit.ui.mount('composer.row', { id: 'voice', order: 90, node: button });

  // take is the dictation going on: see start.
  let take = null;
  // view is what the cockpit says of dictation, null until it has said:
  // { set, provider, providers: [{ id, name, model, live, about, console,
  // env, set, from, hint }], path, settings, languages, problem }.
  let view = null;
  const unset = () => view?.set === false;

  function show() {
    const state = take?.state || 'idle';
    button.dataset.state = state;
    button.setAttribute('aria-pressed', String(state !== 'idle'));
    if (state !== 'listening') button.style.removeProperty('--level');
    button.title = unset() ? 'Dictate: first, the key of a speech-to-text provider' : TITLE;
  }
  const TITLE = button.title;
  show();

  async function refresh() {
    try {
      view = await cockpit.api('/api/voice');
    } catch {
      return; // what it said last stands
    }
    show();
  }
  refresh();

  // ---------------------------------------------------------------- the settings

  // The dialog holds dictation's settings, the cockpit's for every workspace
  // and for the iPhone (PUT /api/voice): the provider that listens and its
  // key, which the cockpit checks with the provider before it keeps it; the
  // languages spoken; what the provider is told of: Russian swearing, the
  // names of the workspace's files, that the talk is of code, and the user's
  // own words. The microphone is this browser's choice, kept in it.
  const MIC = 'voice.mic';
  const provider = h('select', { id: 'voice-provider' });
  const about = h('small', { class: 'voice-hint', id: 'voice-about' });
  const keyLabel = h('span', { class: 'label', id: 'voice-key-label', text: 'API key' });
  const field = h('input', {
    id: 'voice-key', class: 'secret', type: 'text', spellcheck: 'false', autocomplete: 'off', autocapitalize: 'off',
    'data-1p-ignore': true, 'data-lpignore': 'true',
  });
  const forget = h('button', { class: 'act', id: 'voice-key-forget', type: 'button', onclick: () => forgetKey() }, 'Remove key');
  const note = h('p', { class: 'settings-note', id: 'voice-note' });
  const languages = h('input', { id: 'voice-languages', type: 'text', spellcheck: 'false', autocomplete: 'off', autocapitalize: 'off' });
  const check = (name, title, text) => {
    const box = h('input', { type: 'checkbox', id: `voice-${name}` });
    return [box, h('label', { class: 'voice-check' }, box, h('span', null, h('b', { text: title }), h('small', { text })))];
  };
  const [strict, strictRow] = check('strict', 'Only these languages',
    'Nothing is written in another language: no stray English in Russian speech. Off, the provider follows whatever is spoken.');
  const [swearing, swearingRow] = check('swearing', 'Russian swearing and slang',
    'Tells the provider мат and slang when Russian is spoken. Fast or noisy speech with them is otherwise heard as English words, or as the names of files.');
  const [files, filesRow] = check('files', 'Names of the workspace\'s files',
    'composer.js and SessionScreen come out as the workspace spells them.');
  const [coding, codingRow] = check('coding', 'Talk of code',
    'Tells the provider that what is said is instructions to a coding agent.');
  const words = h('textarea', { id: 'voice-words', rows: '4', spellcheck: 'false', placeholder: 'One per line: names, jargon, the words it gets wrong' });
  const mic = h('select', { id: 'voice-mic' });
  const micRow = h('label', { class: 'field' }, h('span', { class: 'label', text: 'Microphone · this browser' }), mic);
  const status = h('div', { class: 'settings-status', id: 'voice-status', role: 'status', 'aria-live': 'polite' });
  const where = h('span', { class: 'settings-path', id: 'voice-path' });
  const save = h('button', { class: 'primary', id: 'voice-save', type: 'submit' }, 'Save');
  const form = h('form', { class: 'settings ticks', id: 'voice-form', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'voice-title', novalidate: true },
    h('header', null, h('span', { class: 'label', id: 'voice-title', text: 'Dictation' }),
      h('button', { class: 'icon', id: 'voice-close', type: 'button', 'aria-label': 'Close', onclick: () => closeSetup() }, '×')),
    h('div', { class: 'settings-body' },
      note,
      h('div', { class: 'field' }, h('label', { class: 'label', for: 'voice-provider', text: 'Provider' }), provider, about),
      h('label', { class: 'field' }, keyLabel, field),
      h('label', { class: 'field' }, h('span', { class: 'label', text: 'Languages' }), languages,
        h('small', { class: 'voice-hint', text: 'Codes, as ru en. None: those of the browser or the phone, and English. The first is the main one, for providers that take one.' })),
      h('div', { class: 'voice-checks' }, strictRow, swearingRow, filesRow, codingRow),
      h('label', { class: 'field' }, h('span', { class: 'label', text: 'Your words' }), words,
        h('small', { class: 'voice-hint', text: 'Told the provider before all else, so that it hears and spells them so.' })),
      micRow,
      status),
    h('footer', null, where, forget,
      h('button', { class: 'act', id: 'voice-cancel', type: 'button', onclick: () => closeSetup() }, 'Cancel'),
      save));
  const overlay = h('div', { class: 'overlay', id: 'voice-setup', hidden: true, onmousedown: (event) => { if (event.target === overlay) closeSetup(); } }, form);
  cockpit.ui.mount('overlays', { id: 'voice-setup', order: 45, node: overlay });
  cockpit.contribute('overlay', { id: 'voice-setup', order: 45, modal: true, isOpen: () => !overlay.hidden, close: () => closeSetup() });
  cockpit.listen(form, 'submit', (event) => {
    event.preventDefault();
    saveSettings();
  });
  // A key pasted is the provider's it was pasted for.
  cockpit.listen(provider, 'change', () => {
    picked = provider.value;
    field.value = '';
    fillProvider();
    say('');
  });
  let dialog = 0; // counts the dialog's openings and closings: an answer for an earlier one is dropped
  let picked = ''; // the provider chosen in the dialog since it opened
  let touched = false; // the user changed something in the form since it opened
  let opener = null; // what had the focus when the dialog opened
  cockpit.listen(form, 'input', () => { touched = true; });
  cockpit.listen(form, 'change', () => { touched = true; });

  function say(text, kind = '') {
    status.textContent = text;
    status.dataset.kind = kind;
  }

  const selected = () => view?.providers?.find((p) => p.id === provider.value) || null;

  // browserLanguages are the languages the cockpit takes when none are set:
  // the browser's, and English.
  function browserLanguages() {
    const codes = (navigator.languages?.length ? navigator.languages : [navigator.language || 'en']).map((tag) => tag.toLowerCase().split(/[-_]/)[0]);
    return [...new Set([...codes, 'en'])].join(' ');
  }

  // fillProviders lists the providers, the one chosen first among them:
  // the one chosen in the dialog, else the one saved.
  function fillProviders() {
    const v = view || {};
    const list = v.providers || [];
    const s = v.settings || {};
    provider.replaceChildren(...list.map((p) => h('option', {
      value: p.id, text: `${p.name} · ${p.model}${p.live ? '' : ' · text when you stop'}${p.set ? ' · key ✓' : ''}`,
    })));
    provider.value = [picked, s.provider, v.provider].find((id) => id && list.some((p) => p.id === id)) || list[0]?.id || '';
    fillProvider();
  }

  function fill() {
    const v = view || {};
    const s = v.settings || { provider: '', languages: [], strict: false, swearing: true, files: true, coding: true, words: [] };
    fillProviders();
    note.textContent = 'The microphone streams through the cockpit to the provider chosen here: its key never leaves the cockpit. These settings serve every workspace and the iPhone.';
    where.textContent = v.path || '';
    languages.value = s.languages.join(' ');
    languages.placeholder = `From this browser: ${browserLanguages()}`;
    strict.checked = !!s.strict;
    swearing.checked = !!s.swearing;
    files.checked = !!s.files;
    coding.checked = !!s.coding;
    words.value = s.words.join('\n');
  }

  // fillProvider shows what is of the provider chosen: what it is, and its key.
  function fillProvider() {
    const p = selected();
    if (!p) {
      about.replaceChildren();
      keyLabel.textContent = 'API key';
      field.disabled = true;
      forget.hidden = true;
      return;
    }
    const environment = p.from === 'environment';
    const variables = p.env.flatMap((name, i) => [i ? ' or ' : '', h('code', { text: name })]);
    about.replaceChildren(`${p.about} Keys: `,
      h('a', { href: p.console, target: '_blank', rel: 'noopener noreferrer', text: p.console.replace(/^https:\/\//, '') }),
      ...(environment
        ? ['. The key is ', ...variables, ` of the cockpit's environment${p.hint ? ` (${p.hint})` : ''}: change it there.`]
        : ['; or set ', ...variables, ' for the cockpit.']));
    keyLabel.textContent = `${p.name} API key`;
    field.disabled = environment;
    field.placeholder = environment ? 'Set by the environment' : p.set ? `Saved key ${p.hint || ''}: paste another to replace it` : 'Paste the key';
    forget.hidden = p.from !== 'file';
  }

  // mics lists the browser's microphones; their names show once the page
  // may use one.
  async function mics() {
    if (!navigator.mediaDevices?.enumerateDevices) {
      micRow.hidden = true;
      return;
    }
    let devices = [];
    try {
      devices = (await navigator.mediaDevices.enumerateDevices()).filter((d) => d.kind === 'audioinput' && d.deviceId && d.deviceId !== 'default');
    } catch { /* the default stands */ }
    const chosen = cockpit.prefs.get(MIC, '');
    mic.replaceChildren(h('option', { value: '', text: 'The system\'s' }),
      ...devices.map((d, i) => h('option', { value: d.deviceId, text: d.label || `Microphone ${i + 1}` })));
    if (chosen && !devices.some((d) => d.deviceId === chosen)) mic.append(h('option', { value: chosen, text: 'The one chosen before, not here now' }));
    mic.value = chosen;
  }

  async function openSetup(message = '') {
    if (cockpit.has('overlays')) cockpit.use('overlays').closeTop?.();
    const opening = ++dialog;
    if (overlay.hidden) {
      const active = document.activeElement;
      opener = active && active !== document.body && !overlay.contains(active) ? active : null;
    }
    overlay.hidden = false;
    picked = '';
    touched = false;
    field.value = '';
    save.disabled = false;
    fill();
    mics();
    say(message, message ? 'error' : '');
    const first = () => (selected() && !selected().set && !field.disabled ? field : provider);
    const focused = first();
    focused.focus();
    await refresh();
    if (opening !== dialog) return;
    // What the user wrote or ticked meanwhile stays: the settings saved
    // fill the form only while it is untouched, and the focus moves only
    // from where the dialog put it.
    if (touched) fillProviders();
    else fill();
    if (view?.problem && !message) say(`${view.problem}: saving replaces it.`, 'warn');
    const active = document.activeElement;
    if (!overlay.contains(active) || (!touched && active === focused)) first().focus();
  }

  function closeSetup() {
    dialog++;
    const had = overlay.contains(document.activeElement);
    overlay.hidden = true;
    // A pasted key never outlives the dialog.
    field.value = '';
    save.disabled = false;
    // The focus goes back to what opened the dialog.
    if (had) {
      if (opener?.isConnected) opener.focus({ preventScroll: true });
      if (overlay.contains(document.activeElement)) document.activeElement.blur();
    }
    opener = null;
  }

  async function saveSettings() {
    const p = selected();
    const value = field.disabled ? '' : field.value.trim();
    const body = {
      settings: {
        provider: provider.value,
        languages: languages.value.split(/[\s,;]+/).filter(Boolean),
        strict: strict.checked, swearing: swearing.checked, files: files.checked, coding: coding.checked,
        words: words.value.split('\n').map((w) => w.trim()).filter(Boolean),
      },
    };
    if (value && p) body.keys = { [p.id]: value };
    const opening = dialog;
    save.disabled = true;
    say(value && p ? `Checking the key with ${p.name}…` : 'Saving…');
    try {
      view = await cockpit.api('/api/voice', { method: 'PUT', body });
    } catch (error) {
      if (opening !== dialog) return;
      save.disabled = false;
      say(error.message, 'error');
      return;
    }
    cockpit.prefs.set(MIC, mic.value || null);
    show();
    if (opening !== dialog) return;
    const now = view.providers?.find((q) => q.id === view.provider);
    if (!view.set) {
      save.disabled = false;
      field.value = '';
      fill();
      say(`Saved. ${now?.name || 'The provider'} still needs a key.`, 'warn');
      field.focus();
      return;
    }
    closeSetup();
    toast(value ? `Dictation listens with ${now?.name}, in every workspace: hold the microphone and talk.` : `Dictation's settings are saved, for every workspace and the iPhone: ${now?.name} listens.`, 'info', 'voice');
  }

  async function forgetKey() {
    const p = selected();
    if (!p) return;
    const opening = dialog;
    try {
      view = await cockpit.api(`/api/voice?provider=${encodeURIComponent(p.id)}`, { method: 'DELETE' });
      show();
    } catch (error) {
      if (opening === dialog) say(error.message, 'error');
      return;
    }
    if (opening !== dialog) return;
    picked = p.id;
    fill();
    say(`${p.name}'s saved key is removed; the settings stay.`);
  }

  cockpit.commands.register({ name: 'voice', help: 'Dictation\'s settings: the provider and its key, the languages, swearing, your words, the microphone', order: 260, run: () => openSetup() });
  cockpit.contribute('settings.section', {
    id: 'voice', title: 'Dictation', order: 40,
    render: () => h('div', { class: 'set-row' },
      h('p', { class: 'set-note', text: view?.set === false
        ? 'No key of a speech-to-text provider yet: the microphone button asks for one.'
        : 'The provider that listens and its key, the languages spoken, Russian swearing, your own words and the microphone: for every workspace and the iPhone. Right-click the microphone button, or /voice, opens them too.' }),
      h('div', null, h('button', { class: 'act', id: 'voice-open-settings', type: 'button', onclick: () => openSetup() }, 'Dictation settings…'))),
  });

  // ---------------------------------------------------------------- a take

  // start opens the microphone and the stream at once, each in its own
  // time: what the microphone hears before the stream is open waits for it,
  // and the cockpit keeps what comes before the provider listens.
  function start() {
    if (!navigator.mediaDevices?.getUserMedia || !window.AudioWorkletNode) {
      toast('The browser gives the microphone only to a secure page: open the cockpit over https, or on localhost.', 'error', 'voice');
      return;
    }
    const value = input.value;
    const from = input.selectionStart ?? value.length;
    const to = input.selectionEnd ?? value.length;
    // Made within the press: Safari keeps an audio context made later still.
    const context = new AudioContext();
    const t = {
      state: 'starting', original: value, before: value.slice(0, from), after: value.slice(to), written: value,
      final: '', partial: '', caret: null, focused: document.activeElement === input, then: null,
      context, stream: null, source: null, node: null, closing: false,
      socket: null, open: false, everOpen: false, queue: [], provider: '', done: false,
      opened: signal(), fin: signal(), flushed: null,
      ended: false, failed: false, detached: false, cancelled: false,
    };
    take = t;
    show();
    connect(t);
    // The provider hears best what the browser did not clean up; the
    // microphone is the one chosen in the settings, if it is there.
    const chosen = cockpit.prefs.get(MIC, '');
    navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: false, noiseSuppression: false, autoGainControl: false, ...(chosen ? { deviceId: { ideal: chosen } } : {}) } })
      .then((stream) => listen(t, stream), (error) => fail(t, refused(error)));
  }

  async function listen(t, stream) {
    t.stream = stream;
    if (t.ended) return release(t);
    try {
      await t.context.audioWorklet.addModule(new URL('pcm-worklet.js', import.meta.url));
      if (t.ended) return release(t);
      // Not awaited: where the browser wants a gesture first, the promise
      // waits for it, and the press's end is one (see the button).
      if (t.context.state !== 'running') t.context.resume().catch(() => {});
      t.source = t.context.createMediaStreamSource(stream);
      t.node = new AudioWorkletNode(t.context, 'kou-voice-pcm', { numberOfOutputs: 1, outputChannelCount: [1], processorOptions: { rate: RATE } });
      t.node.port.onmessage = ({ data }) => heard(t, data);
      t.source.connect(t.node);
      // Its output is silence; joined to the speakers, the graph pulls it.
      t.node.connect(t.context.destination);
    } catch (error) {
      return fail(t, `The microphone could not be read: ${error?.message || error}`);
    }
    if (t.state === 'starting') {
      t.state = 'listening';
      show();
    }
  }

  function heard(t, { pcm, level, flushed }) {
    if (pcm) {
      if (t.open) t.socket.send(pcm);
      else if (!t.ended) t.queue.push(pcm);
    }
    if (level != null && take === t && t.state === 'listening') {
      button.style.setProperty('--level', Math.min(1, Math.sqrt(level) * 1.5).toFixed(2));
    }
    if (flushed) t.flushed?.();
  }

  function connect(t) {
    const tags = navigator.languages?.length ? navigator.languages : [navigator.language || 'en'];
    const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
    const path = cockpit.wsPath(cockpit.host.workspace(), `/voice/socket?languages=${encodeURIComponent(tags.join(','))}`);
    let socket;
    try {
      socket = new WebSocket(`${scheme}://${location.host}${path}`);
    } catch (error) {
      return fail(t, `The cockpit could not be reached: ${error?.message || error}`);
    }
    socket.binaryType = 'arraybuffer';
    t.socket = socket;
    // A socket that neither opens nor fails, as behind a proxy that holds
    // it, would leave the take listening to no one.
    setTimeout(() => { if (!t.everOpen && !t.ended) fail(t, 'The cockpit could not be reached: dictation\'s stream did not open in 10 s.'); }, 10_000);
    socket.onopen = () => {
      for (const pcm of t.queue.splice(0)) socket.send(pcm);
      t.open = t.everOpen = true;
      t.opened.fire(true);
    };
    socket.onmessage = ({ data }) => { if (typeof data === 'string') answer(t, data); };
    socket.onclose = (event) => {
      t.open = false;
      t.opened.fire(false);
      t.fin.fire();
      if (!t.done) fail(t, t.everOpen ? `Dictation's stream closed (${event.code}${event.reason ? `: ${event.reason}` : ''}).` : 'The cockpit could not be reached.');
    };
  }

  // answer takes what the cockpit says: the provider listens; all the words
  // it is sure of and its guess at those after them, sent anew each time;
  // that it is sure of them all; or what went wrong.
  function answer(t, text) {
    let message;
    try {
      message = JSON.parse(text);
    } catch {
      return;
    }
    switch (message.type) {
      case 'ready':
        t.provider = message.name || '';
        break;
      case 'text':
        t.final = message.final || '';
        t.partial = message.partial || '';
        write(t);
        break;
      case 'done':
        t.done = true;
        t.fin.fire();
        break;
      case 'error':
        // No key, or one the provider refuses: the take ends quietly, and
        // the dialog asks for a key.
        if (message.setup && !t.ended) {
          t.cancelled = true;
          end(t);
          openSetup(message.message || '');
          return;
        }
        fail(t, message.message || 'Dictation failed.');
        break;
    }
  }

  // write puts the words so far where the caret was, spaced from what is
  // before and after them.
  function write(t) {
    if (t.ended) return;
    if (input.value !== t.written) return detach(t);
    const words = (t.final + t.partial).replace(/^\s+/, '');
    const lead = words && t.before && !/\s$/.test(t.before) ? ' ' : '';
    const trail = words && t.after && !/^[\s.,;:!?)\]}]/.test(t.after) ? ' ' : '';
    const value = t.before + lead + words + trail + t.after;
    t.written = value;
    t.caret = t.before.length + lead.length + words.length;
    if (value !== input.value) composer.set(value);
    if (document.activeElement === input) input.setSelectionRange(t.caret, t.caret);
  }

  // stop ends a take once the provider is sure of what it heard: it listens
  // to the tail, sends what the worklet holds, lets the microphone go and
  // asks for the final words. then, if given, runs the prompt after.
  function stop(t, then) {
    if (then) t.then = then;
    if (t.ended || t.state === 'finishing') return;
    t.state = 'finishing';
    show();
    finish(t);
  }

  async function finish(t) {
    if (t.node) {
      await sleep(TAIL);
      if (!t.ended) {
        await within(new Promise((resolve) => {
          t.flushed = resolve;
          t.node.port.postMessage('flush');
        }), 300);
      }
      release(t);
      const opened = !t.ended && (await within(t.opened.promise, 10_000));
      if (t.ended) return;
      // What was heard went nowhere: the take fails rather than end as if
      // all was said, and the prompt does not run.
      if (!opened) return fail(t, 'The cockpit could not be reached: dictation\'s stream did not open.');
      t.socket.send(JSON.stringify({ type: 'finish' }));
      await within(t.fin.promise, WAIT);
      if (t.ended) return;
      if (!t.done) return fail(t, `${t.provider || 'The provider'} did not give the last words in time.`);
    }
    if (t.ended) return;
    end(t);
    composer.saveDraft();
    if (t.focused) {
      composer.focus();
      if (t.caret != null) input.setSelectionRange(t.caret, t.caret);
    }
    if (t.then?.submit) composer.submit({ force: t.then.force });
  }

  // end closes the stream and lets the microphone go.
  function end(t) {
    if (t.ended) return;
    t.ended = true;
    release(t);
    const socket = t.socket;
    if (socket) {
      socket.onopen = socket.onmessage = socket.onclose = null;
      socket.close(1000);
    }
    t.open = false;
    t.opened.fire(false);
    t.fin.fire();
    if (take === t) take = null;
    show();
  }

  // release lets the microphone go; once more when it came after the end.
  function release(t) {
    if (t.node) t.node.port.onmessage = null;
    try {
      t.source?.disconnect();
      t.node?.disconnect();
    } catch { /* gone with the context */ }
    for (const track of t.stream?.getTracks() || []) track.stop();
    if (!t.closing) {
      t.closing = true;
      t.context.close().catch(() => {});
    }
  }

  // fail ends a take with what went wrong; what it wrote stays.
  function fail(t, message) {
    if (t.failed || t.cancelled || t.detached) return;
    t.failed = true;
    const ended = t.ended;
    end(t);
    if (!ended && t.caret != null) composer.saveDraft();
    toast(message, 'error', 'voice');
  }

  // detach ends a take whose words were typed over: the text is the user's.
  function detach(t) {
    if (t.ended) return;
    t.detached = true;
    end(t);
  }

  // cancel ends a take and puts back what the composer held before it,
  // unless something was typed since.
  function cancel(t) {
    if (t.ended) return;
    t.cancelled = true;
    end(t);
    if (input.value !== t.written) return;
    composer.set(t.original);
    if (t.focused) {
      composer.focus();
      input.setSelectionRange(t.before.length, t.original.length - t.after.length);
    }
  }

  // ---------------------------------------------------------------- the button

  // A press starts a take; let go after HOLD, it stops. A shorter press is
  // a click, and the next one stops. The press keeps the focus where it was:
  // on a phone, the keyboard stays as it is.
  let press = null; // { id, at }
  let pressed = 0; // when the last press ended: the click that follows is its
  cockpit.listen(button, 'pointerdown', (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    if (take) {
      if (take.state !== 'finishing') stop(take);
      return;
    }
    // Without a key it asks for one, and listens to nothing.
    if (unset()) return openSetup();
    button.setPointerCapture?.(event.pointerId);
    press = { id: event.pointerId, at: performance.now() };
    start();
  });
  function letGo(event, cancelled) {
    pressed = performance.now();
    if (!press || event.pointerId !== press.id) return;
    const held = pressed - press.at;
    press = null;
    // A touch is no gesture until it ends: Safari lets the audio run then.
    if (take?.context.state === 'suspended') take.context.resume().catch(() => {});
    if (take && (cancelled || held >= HOLD)) stop(take);
  }
  cockpit.listen(button, 'pointerup', (event) => letGo(event, false));
  cockpit.listen(button, 'pointercancel', (event) => letGo(event, true));
  cockpit.listen(button, 'mousedown', (event) => event.preventDefault());
  cockpit.listen(button, 'touchstart', (event) => event.preventDefault(), { passive: false });
  // A right click opens the settings; on a phone a long press is a hold.
  cockpit.listen(button, 'contextmenu', (event) => {
    event.preventDefault();
    if (!touch.matches && !take) openSetup();
  });
  // The keyboard, and assistive technologies, click.
  cockpit.listen(button, 'click', () => {
    if (performance.now() - pressed < 600) return;
    if (!take) {
      if (unset()) openSetup();
      else start();
    } else if (take.state !== 'finishing') stop(take);
  });
  // Hovered without a key, it learns whether there is one now, as one may
  // have been set elsewhere.
  cockpit.listen(button, 'pointerenter', (event) => {
    if (event.pointerType === 'mouse' && !take && unset()) refresh();
  });

  // ---------------------------------------------------------------- keys

  cockpit.hooks.tap('composer.key', (event) => {
    if (!take || event.key !== 'Enter' || event.shiftKey || event.isComposing) return false;
    const force = event.metaKey || event.ctrlKey;
    // A touch screen's Enter adds a line, which is typing.
    if (touch.matches && !force) return false;
    event.preventDefault();
    stop(take, { submit: true, force });
    return true;
  }, { order: 5 });
  cockpit.keys.register({ key: 'Escape', global: true, priority: 60, when: () => !!take, run: () => cancel(take) });

  // Typing takes over: the take stops, and its words stay as they are.
  cockpit.listen(input, 'input', () => { if (take) detach(take); });
  cockpit.on('session:leave', () => { if (take) detach(take); });
  cockpit.listen(document, 'visibilitychange', () => { if (document.hidden && take) stop(take); });
  cockpit.onDispose(() => { if (take) detach(take); });
}

function signal() {
  let fire;
  const promise = new Promise((resolve) => { fire = resolve; });
  return { promise, fire };
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const within = (promise, ms) => Promise.race([promise, sleep(ms)]);

function refused(error) {
  switch (error?.name) {
    case 'NotAllowedError':
    case 'SecurityError':
      return 'The microphone is not allowed for this page: allow it in the browser\'s settings for the site.';
    case 'NotFoundError':
    case 'OverconstrainedError':
      return 'No microphone was found.';
    case 'NotReadableError':
    case 'AbortError':
      return 'The microphone is busy: another app may be using it.';
    default:
      return `The microphone could not be opened: ${error?.message || error}`;
  }
}
