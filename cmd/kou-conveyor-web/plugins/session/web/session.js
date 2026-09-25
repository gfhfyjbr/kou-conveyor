// session: the cockpit's model. The server owns runs; the page is a view
// onto one session at a time. This plugin holds that view and everything
// around it — the configuration, the workspaces, the session list, the run
// in progress and its event stream, the address — and provides them as the
// session service. It shows nothing itself: the other plugins draw what it
// holds, redrawing on "render", and follow it through events:
//
//   session:view (v, { reload, previous })  another view is shown
//   session:leave (v)                       a view is about to be left
//   session:entry (v, entry)                an entry came or changed
//   session:dirty (v, id?)                  an entry (all, without id) needs drawing again
//   session:loaded (v, data)                a session was read from the server
//   session:finish (v, event)               a run ended
//   session:changes (v, update)             a run changed files
//   session:queue (v)                       what waits for the agent changed
//   session:gone (v)                        the session was deleted elsewhere
//   session:before-run (v)                  a run is about to start
//   session:sessions (list), session:workspaces (list), session:workspace (id), session:config (config)
//
// and the public entry, session and finish events plugins had from the
// start. Every asynchronous result is checked against the view it was
// started for, so switching sessions mid-flight can never mix two sessions
// or resurrect a stale one. The state is kept across versions of this
// plugin: loaded anew, it picks up the session in view and its run.

// What Continue sends; the transcript keeps it like any other prompt.
export const CONTINUE = 'Continue where you left off.';

const PHASE_LABEL = {
  idle: 'Idle', starting: 'Starting', running: 'Running', stopping: 'Stopping',
  done: 'Done', failed: 'Failed', stopped: 'Stopped', external: 'In use',
};

export default function activate(cockpit) {
  const { prefs, uuid, fmt } = cockpit;
  const api = cockpit.api;
  const wsPath = cockpit.wsPath;
  const toast = (text, kind = 'info', key = '') => cockpit.toast(text, kind, key);
  const copy = (text, label) => cockpit.copy(text, label);
  const hot = cockpit.hot.data;
  const state = hot.state ??= {
    config: null,
    workspaces: [], // folders the server runs agents in; the first is where it started
    ws: null, // the workspace this tab shows
    workspaceTicket: 0,
    sessions: [],
    sessionTicket: 0,
    view: null,
    epoch: 0,
    outcome: null, // { kind, at } of the last finished run, shown briefly
    unseenOutcome: null,
    stream: 'idle', // the run stream: idle, connecting, live, reconnecting
    booted: false,
    configured: false, // the configuration was asked for
  };
  const view = () => state.view;
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);

  // ---------------------------------------------------------------- views

  const workspaceHash = (ws) => (ws ? `#/w/${ws}` : '#/');
  const sessionHash = (ws, id) => (ws ? `#/w/${ws}/s/${id}` : `#/s/${id}`);

  function currentWorkspace() {
    return state.workspaces.find((w) => w.id === state.ws) || null;
  }

  function newView(id, fresh) {
    return {
      epoch: ++state.epoch,
      ws: state.ws,
      id,
      fresh, // not persisted yet
      title: '',
      entries: new Map(),
      order: [],
      users: new Map(), // user entry id → exchange number
      usage: null,
      run: null, // { id, phase: starting|running|stopping, started, activity, source, stopAsked }
      logs: [],
      expanded: new Map(),
      loading: false,
      failed: '',
      size: -1, // bytes of the session file this view reflects; -1 before a load
      external: false, // another process is running the session
      pinned: false,
      interrupted: false, // the last run ended before the agent finished
      resumeDismissed: false,
      gone: false, // deleted elsewhere while this view showed it
      renamed: false, // the title is the user's rather than the first prompt
      edit: null, // the prompt being edited (the edit plugin's)
      editAfterStop: false, // Esc Esc while stopping: edit the last prompt once stopped
      queue: { items: [], paused: false }, // what waits for the agent, kept by the server
      // model is the model chosen for the session's next prompts; '' leaves
      // them the model its last prompt ran with, else the connection's.
      model: '',
      arrived: new Set(), // forced prompts the agent just read, to light up once
    };
  }

  function firstPrompt(v) {
    for (const id of v.order) {
      const e = v.entries.get(id);
      if (e.kind === 'user') return (e.text || '').replace(/\s+/g, ' ').trim().slice(0, 80);
    }
    return '';
  }

  function lastPrompt(v) {
    for (let i = v.order.length - 1; i >= 0; i--) {
      const entry = v.entries.get(v.order[i]);
      if (entry?.kind === 'user') return entry;
    }
    return null;
  }

  // lastModel is the model the session's last prompt ran with, if known.
  function lastModel(v) {
    for (let i = (v?.order.length || 0) - 1; i >= 0; i--) {
      const e = v.entries.get(v.order[i]);
      if (e.kind === 'user' && e.model && e.state !== 'undelivered') return e.model;
    }
    return '';
  }

  // previousModel is the model of the prompt before the one with the given ID.
  function previousModel(v, id) {
    let model = '';
    for (const other of v.order) {
      if (other === id) return model;
      const e = v.entries.get(other);
      if (e.kind === 'user' && e.model && e.state !== 'undelivered') model = e.model;
    }
    return model;
  }

  // promptNumbered returns prompt n of the view, numbered as the transcript
  // shows them.
  function promptNumbered(v, arg) {
    const n = Number.parseInt(String(arg).replace(/^#/, ''), 10);
    if (!(n >= 1)) return { error: 'Give the number of a prompt, as shown beside it.' };
    for (const [id, number] of v.users) {
      if (number === n) return { entry: v.entries.get(id) };
    }
    return { error: `There is no prompt ${n}.` };
  }

  // The model and effort of the next prompt come from their plugins, when
  // they are there.
  const nextModel = (v) => service('models')?.next?.(v) ?? (v ? v.model || lastModel(v) : '');
  const defaultModel = () => service('models')?.default?.() ?? (currentWorkspace()?.connection?.model || state.config?.model || '');
  const effort = () => service('effort')?.current?.() || state.config?.thinking || '';

  // summary is what plugins see of a view.
  function summary(v = view()) {
    if (!v) return null;
    return {
      id: v.id, ws: v.ws, fresh: v.fresh, title: v.title || firstPrompt(v), interrupted: !!v.interrupted,
      running: !!v.run, external: !!v.external, activity: v.run?.activity || '', usage: v.usage ? { ...v.usage } : null,
      prompts: v.users.size, entries: v.order.length, queued: v.queue.items.length, paused: !!v.queue.paused,
      pinned: !!v.pinned, gone: !!v.gone, loading: !!v.loading, editing: !!v.edit,
    };
  }

  function copyEntry(entry) {
    return entry && { ...entry, tool: entry.tool && { ...entry.tool } };
  }

  // touch has an entry (every entry, without id) drawn again, and the page.
  function touch(v, id) {
    if (v !== view()) return;
    cockpit.emit('session:dirty', v, id);
    cockpit.render();
  }

  function upsert(v, entry) {
    if (!entry?.id) return;
    entry.text ??= '';
    if (!v.entries.has(entry.id)) {
      v.order.push(entry.id);
      if (entry.kind === 'user') v.users.set(entry.id, v.users.size + 1);
    }
    v.entries.set(entry.id, entry);
    if (v === view()) {
      cockpit.emit('session:entry', v, entry);
      touch(v, entry.id);
      cockpit.emit('entry', copyEntry(entry), summary(v));
    }
  }

  // Some entries render differently with and without a live run: unfinished
  // tools read as interrupted, and prompts can be branched only between runs.
  function refreshTools(v) {
    for (const entry of v.entries.values()) {
      if (entry.kind === 'user' || (entry.kind === 'tool' && !['done', 'failed', 'canceled'].includes(entry.tool?.state))) touch(v, entry.id);
    }
  }

  function runPhase(v = view()) {
    if (!v) return 'idle';
    if (v.run) return v.run.phase;
    if (v.external) return 'external';
    if (state.outcome && Date.now() - state.outcome.at < 5000) return state.outcome.kind;
    return 'idle';
  }

  function setStream(status) {
    if (state.stream === status) return;
    state.stream = status;
    cockpit.render();
  }

  function log(text, v = view()) {
    if (!v) return;
    v.logs.push(String(text));
    if (v.logs.length > 400) v.logs.splice(0, v.logs.length - 400);
    if (v === view()) cockpit.render();
  }

  // ---------------------------------------------------------------- sessions list

  let sessionsPoll = 0;

  async function refreshSessions() {
    const ticket = ++state.sessionTicket;
    const ws = state.ws;
    let list;
    try {
      list = await api(wsPath(ws, '/sessions'));
    } catch {
      return;
    }
    // A newer refresh, or another workspace, landed meanwhile.
    if (ticket !== state.sessionTicket || ws !== state.ws) return;
    state.sessions = list;
    cockpit.emit('session:sessions', list);
    const v = view();
    if (!v || v.ws !== ws) return;
    const mine = list.find((s) => s.id === v.id);
    if (!mine && !v.fresh && !v.run && !v.loading && !v.gone) {
      v.gone = true;
      // The banner offers to take the composer's prompt to a new session.
      cockpit.emit('session:gone', v);
      cockpit.render();
    }
    if (!mine || v.loading) return;
    if (v.fresh) v.fresh = false;
    // Another tab started a run, or a terminal appended to the session: the
    // file only grows, so a size other than the one this view read means there
    // is more to show.
    const stale = !v.run && (mine.run_id || v.external || (v.size >= 0 && mine.size !== v.size));
    if (stale) openSession(v.id, { push: false, quiet: true });
  }

  function schedulePoll() {
    clearTimeout(sessionsPoll);
    // A session run by another process is followed more closely.
    const every = view()?.external ? 2000 : 5000;
    if (document.visibilityState === 'visible') {
      sessionsPoll = setTimeout(() => Promise.all([refreshSessions(), refreshWorkspaces()]).finally(schedulePoll), every);
    }
  }
  cockpit.onDispose(() => clearTimeout(sessionsPoll));

  // ---------------------------------------------------------------- navigation

  function detach(v) {
    if (v?.run?.source) {
      v.run.source.close();
      v.run.source = null;
    }
  }

  function showSessions() {
    service('layout')?.show?.('sessions');
  }

  function switchTo(v, { reload = false } = {}) {
    const previous = view();
    // A menu acts on the session it was opened for; it must not outlive it.
    if (!reload) service('menu')?.close?.();
    if (previous && previous !== v) {
      previous.abort?.abort();
      detach(previous);
      if (!reload) cockpit.emit('session:leave', previous);
    }
    state.view = v;
    setStream('idle');
    cockpit.emit('session:view', v, { reload, previous });
    touch(v);
  }

  function newSession({ push = true } = {}) {
    if (push) showSessions();
    switchTo(newView(uuid(), true));
    const hash = workspaceHash(state.ws);
    if (push && location.hash !== hash) history.pushState(null, '', hash);
    service('composer')?.focus?.();
    cockpit.emit('session', summary());
  }

  // ensureView gives the page a view to come back to, for views that show
  // something else.
  function ensureView() {
    if (!view() && state.configured) newSession({ push: false });
  }

  async function openSession(id, { push = true, quiet = false } = {}) {
    const previous = view();
    if (quiet && (previous?.id !== id || previous.ws !== state.ws)) return;
    const v = newView(id, false);
    v.model = prefs.get(`model.${v.ws || 'default'}.${v.id}`, '') || '';
    if (quiet && previous?.id === id) {
      v.expanded = previous.expanded;
      v.logs = previous.logs;
      v.edit = previous.edit;
    }
    v.loading = !quiet;
    const controller = new AbortController();
    v.abort = controller;
    if (push && !quiet) showSessions();
    if (!quiet) switchTo(v);
    const hash = sessionHash(v.ws, id);
    if (push && location.hash !== hash) history.pushState(null, '', hash);

    let data;
    try {
      data = await api(wsPath(v.ws, `/sessions/${encodeURIComponent(id)}`), { signal: controller.signal });
    } catch (error) {
      if (error.name === 'AbortError' || (!quiet && v !== view())) return;
      if (quiet) return; // keep showing what we have
      v.loading = false;
      if (error.status === 404) {
        toast('Session not found', 'error');
        newSession({ push: false });
        history.replaceState(null, '', workspaceHash(state.ws));
        return;
      }
      v.failed = error.message;
      touch(v);
      return;
    }
    // A quiet reload replaces the current view only if nothing happened in the
    // meantime; otherwise its result is simply dropped.
    if (quiet) {
      if (view() !== previous || previous.run) return;
      if (!data.run && data.size === previous.size && !!data.external === previous.external) return;
      switchTo(v, { reload: true });
    } else if (v !== view()) {
      return;
    }
    v.loading = false;
    v.title = data.title || '';
    v.renamed = !!data.renamed;
    v.usage = data.usage;
    v.size = data.size ?? 0;
    v.external = !!data.external;
    v.pinned = !!data.pinned;
    v.interrupted = !!data.interrupted;
    v.queue = data.queue || { items: [], paused: false };
    if (quiet) v.resumeDismissed = previous.resumeDismissed;
    for (const entry of data.entries) upsert(v, entry);
    // An edit outlives a reload only while its prompt is there and nothing
    // runs; the edit plugin decides.
    cockpit.emit('session:loaded', v, data);
    touch(v);
    if (data.run) attach(v, data.run);
    cockpit.emit('session:sessions', state.sessions);
    cockpit.emit('session:queue', v);
    cockpit.emit('session', summary(v));
  }

  // routeSessions follows the address: #/w/<workspace>/s/<session>,
  // #/w/<workspace> for a new session there, and #/s/<session> from before
  // workspaces, which names a session in the first workspace. Addresses of
  // other views are theirs (see "routes").
  function routeSessions({ push = false } = {}) {
    if (!state.configured) return;
    showSessions();
    const scoped = /^#\/w\/([A-Za-z0-9-]{1,80})(?:\/s\/([A-Za-z0-9-]{1,128}))?$/.exec(location.hash);
    const legacy = /^#\/s\/([A-Za-z0-9-]{1,128})$/.exec(location.hash);
    let ws = state.ws;
    let id = null;
    if (scoped) [, ws, id = null] = scoped;
    else if (legacy) [ws, id] = [state.workspaces[0]?.id || null, legacy[1]];
    if (state.workspaces.length && !state.workspaces.some((w) => w.id === ws)) {
      toast('That workspace is not in the list any more', 'error');
      ws = state.ws;
      id = null;
      history.replaceState(null, '', workspaceHash(ws));
    }
    if (ws !== state.ws) selectWorkspace(ws);
    const current = view();
    if (id) {
      if (current?.id !== id || current.ws !== ws || current.fresh) openSession(id, { push });
    } else if (!current || !current.fresh || current.ws !== ws) {
      newSession({ push: false });
    }
  }

  cockpit.routes.register({ id: 'sessions', priority: 0, match: (hash) => ({ hash }), enter: () => routeSessions() });

  // ---------------------------------------------------------------- runs

  function attach(v, run) {
    detach(v);
    const source = new EventSource(`/api/runs/${encodeURIComponent(run.id)}/events`);
    v.run = {
      id: run.id, phase: run.stopping ? 'stopping' : 'running',
      started: run.started_at ? Date.parse(run.started_at) : Date.now(),
      activity: run.activity || (run.stopping ? 'Stopping' : run.compact ? 'Compacting context' : 'Working'), source,
      compact: !!run.compact,
    };
    refreshTools(v);
    setStream('connecting');
    source.onopen = () => { if (v === view() && v.run?.source === source) setStream('live'); };
    source.onmessage = (message) => {
      if (v !== view() || v.run?.source !== source) {
        source.close();
        return;
      }
      let event;
      try { event = JSON.parse(message.data); } catch { return; }
      cockpit.safely(() => handleEvent(v, event));
    };
    source.onerror = () => {
      if (v.run?.source !== source) return;
      if (source.readyState !== EventSource.CLOSED) {
        setStream('reconnecting');
        return;
      }
      // The server no longer knows the run (it restarted or the run expired):
      // the session file is the truth now.
      source.close();
      v.run = null;
      if (v !== view()) return;
      setStream('idle');
      openSession(v.id, { push: false, quiet: true });
    };
    cockpit.render();
  }

  function handleEvent(v, event) {
    switch (event.type) {
      case 'entry':
        // A forced prompt shows up where the agent read it, and lights up.
        if (event.entry.forced && !v.entries.has(event.entry.id)) v.arrived.add(event.entry.id);
        upsert(v, event.entry);
        if (!v.title && event.entry.kind === 'user') v.title = firstPrompt(v);
        break;
      case 'queue':
        setQueue(v, event.queue);
        break;
      case 'status':
        if (event.activity) v.run.activity = event.activity;
        if (event.usage) v.usage = event.usage;
        if (event.stopping) v.run.phase = 'stopping';
        cockpit.render();
        break;
      case 'changes':
        cockpit.emit('session:changes', v, event.changes);
        break;
      case 'log':
        log(event.text, v);
        break;
      case 'done':
        finish(v, event);
        break;
      default:
        cockpit.emit('session:event', v, event);
    }
  }

  function finish(v, event) {
    const compact = !!v.run?.compact;
    detach(v);
    v.run = null;
    v.fresh = false;
    if (event.queue) setQueue(v, event.queue);
    if (event.size) v.size = event.size;
    if (event.usage) v.usage = event.usage;
    const kind = event.stopped ? 'stopped' : event.error ? 'failed' : 'done';
    // A compaction runs no prompt: whether the last one finished stays as it was.
    if (!compact) v.interrupted = kind !== 'done';
    if (compact && kind === 'done') compacted(v);
    cockpit.emit('finish', { kind, compact, view: summary(v) });
    v.resumeDismissed = false;
    state.outcome = { kind, at: Date.now() };
    if (document.visibilityState !== 'visible') state.unseenOutcome = kind === 'failed' ? 'failed' : 'done';
    setStream('idle');
    refreshTools(v);
    cockpit.render();
    refreshSessions();
    cockpit.emit('session:finish', v, event);
    // The next queued prompt took over the session: follow it.
    if (event.next && v === view()) {
      const left = v.queue.items.length;
      state.outcome = null;
      attach(v, { id: event.next.id, started_at: event.next.started_at });
      toast(left ? `Running the next queued message · ${left} more after it` : 'Running the next queued message', 'info', 'queue');
    } else if (event.queue?.paused && event.queue.items.length && v === view()) {
      toast(`The queue is paused: the run ${kind === 'failed' ? 'failed' : 'was stopped'}. Resume it when you are ready.`, 'warn', 'queue');
    }
  }

  // compacted says how a compaction that finished went: its notice holds the
  // summary, or the runner found nothing new to summarize.
  function compacted(v) {
    const id = v.order[v.order.length - 1] || '';
    const notice = id.startsWith('compaction:') ? v.entries.get(id) : null;
    if (!notice) toast('Nothing to compact: the agent has not answered since the last compaction.');
    else if (!notice.detail) toast(notice.text, 'error');
    else toast('Context compacted. The next prompt starts from the summary; click the notice to read it.');
  }

  // runBlocked says why the view cannot start a run now, or ''.
  function runBlocked(v = view()) {
    if (v.loading) return 'The session is still loading';
    if (v.run) return 'A run is in progress. Press Esc twice to stop it.';
    if (v.external) return 'This session is running in another window or terminal.';
    if (v.gone) return 'This session was deleted. Start a new session to run the prompt.';
    if (currentWorkspace()?.missing) return 'The workspace folder no longer exists.';
    return '';
  }

  // submit runs a prompt: the composer's (fromComposer: the composer is
  // cleared, and its images go with the prompt) or a given text (Continue,
  // an edited prompt). rewind names the prompt an edited one replaces, and
  // undo puts the view back if the run does not start. The request passes
  // through the "run.request" hook on its way to the server.
  async function submit(given, { rewind = '', undo = null, images = null, model: givenModel = null, fromComposer = false } = {}) {
    const v = view();
    const text = String(given ?? '').trim();
    if (!text) return;
    const blocked = runBlocked(v);
    if (blocked) return toast(blocked);
    // A prompt at the end leaves an edit further up behind.
    cockpit.emit('session:before-run', v);

    const messageID = uuid();
    // The prompt runs with the model chosen for the session, else the one its
    // last prompt ran with, else the connection's; an edited one, unless a
    // model was chosen, with the one it ran with.
    const model = givenModel ?? nextModel(v);
    const pictures = service('images');
    const composer = fromComposer ? service('composer') : null;
    // The images the prompt brings: the composer's that its text names, or
    // those an edit keeps.
    const sent = fromComposer ? (pictures?.take?.(text) || []) : (images || []);
    pictures?.remember?.(messageID, sent);
    const entry = {
      id: `input:${messageID}`, kind: 'user', text, state: 'pending', at: new Date().toISOString(), model: model || defaultModel() || undefined,
      images: sent.length && pictures ? sent.map((att) => pictures.info(att)) : undefined,
    };
    upsert(v, entry);
    if (fromComposer) composer?.clear?.();
    v.interrupted = false;
    // Messages queued while the run starts wait until it has its session.
    let ready;
    v.run = { id: null, phase: 'starting', started: Date.now(), activity: 'Starting', source: null, stopAsked: false, ready: new Promise((resolve) => { ready = resolve; }) };
    state.outcome = null;
    cockpit.render();
    requestAnimationFrame(() => service('timeline')?.scrollToBottom?.());

    let started;
    try {
      let body = {
        prompt: text, session_id: v.id, message_id: messageID, thinking_level: effort() || undefined, resume: !v.fresh,
        rewind: rewind || undefined, model: model || undefined,
        images: sent.length ? await pictures.encode(sent) : undefined,
      };
      body = await cockpit.hooks.runAsync('run.request', body, summary(v));
      started = await api(wsPath(v.ws, '/runs'), { method: 'POST', body });
    } catch (error) {
      ready();
      if (v !== view()) return;
      v.run = null;
      if (undo) {
        undo();
      } else {
        upsert(v, { ...entry, state: 'undelivered' });
        if (fromComposer) composer?.restore?.(text, sent);
      }
      if (error.status === 409 && error.body?.run_id) {
        toast('This session is already running elsewhere. Following that run.');
        openSession(v.id, { push: false, quiet: true });
      } else if (error.status === 409 && error.body?.code === 'prompt_gone') {
        toast('That prompt changed in another window.', 'error');
        openSession(v.id, { push: false, quiet: true });
      } else if (error.status === 410) {
        v.gone = true;
        cockpit.emit('session:gone', v);
        toast('This session was deleted in another window. The prompt is back in the composer.', 'error');
      } else {
        toast(error.message, 'error');
        // A runner that failed to start may have found the session rewound.
        if (undo) openSession(v.id, { push: false, quiet: true });
      }
      cockpit.render();
      return;
    }
    ready();
    // The run lives on the server; if the user moved on, it shows up in the
    // session list and reattaches when they come back.
    if (v !== view()) return refreshSessions();
    const stopAsked = v.run?.stopAsked;
    if (v.fresh) history.replaceState(null, '', sessionHash(v.ws, v.id));
    attach(v, { id: started.run_id, started_at: new Date(v.run.started).toISOString() });
    if (stopAsked) stop();
    refreshSessions();
  }

  // compact asks the runner to summarize the conversation, which frees the
  // context it takes: the next prompt starts from the summary. focus tells
  // the summary what matters most.
  async function compact(focus = '', { fromComposer = false } = {}) {
    const v = view();
    const blocked = runBlocked(v);
    if (blocked) return toast(blocked);
    if (v.fresh) return toast('Nothing to compact yet: the session starts with its first prompt.');
    cockpit.emit('session:before-run', v);
    const composer = fromComposer ? service('composer') : null;
    if (fromComposer) composer?.clear?.({ images: false });
    v.run = { id: null, phase: 'starting', started: Date.now(), activity: 'Compacting context', source: null, stopAsked: false, compact: true };
    state.outcome = null;
    cockpit.render();
    let started;
    try {
      started = await api(wsPath(v.ws, '/runs'), {
        method: 'POST',
        body: {
          compact: true, instructions: String(focus).trim() || undefined, session_id: v.id, thinking_level: effort() || undefined, resume: true,
          model: nextModel(v) || undefined,
        },
      });
    } catch (error) {
      if (v !== view()) return;
      v.run = null;
      if (fromComposer) composer?.restore?.(`/compact ${focus}`.trim(), null);
      if (error.status === 409 && error.body?.run_id) {
        toast('This session is already running elsewhere. Following that run.');
        openSession(v.id, { push: false, quiet: true });
      } else if (error.status === 410) {
        v.gone = true;
        toast('This session was deleted in another window.', 'error');
      } else {
        toast(error.message, 'error');
      }
      cockpit.render();
      return;
    }
    if (v !== view()) return refreshSessions();
    const stopAsked = v.run?.stopAsked;
    attach(v, { id: started.run_id, started_at: new Date(v.run.started).toISOString(), compact: true });
    if (stopAsked) stop();
    refreshSessions();
  }

  async function stop() {
    const v = view();
    const run = v.run;
    if (!run || run.phase === 'stopping') return;
    toast('Stopping the run', 'info', 'esc');
    if (!run.id) {
      run.stopAsked = true; // stop as soon as the server names the run
      run.phase = 'stopping';
      cockpit.render();
      return;
    }
    run.phase = 'stopping';
    run.activity = 'Stopping';
    cockpit.render();
    try {
      await api(`/api/runs/${encodeURIComponent(run.id)}/cancel`, { method: 'POST' });
    } catch (error) {
      if (error.status === 404) return; // it already ended; the stream says so
      if (v.run === run) run.phase = 'running';
      toast(`Could not stop the run: ${error.message}`, 'error');
      cockpit.render();
    }
  }

  function continueRun() {
    const v = view();
    if (!v.interrupted || v.external) return toast('The last run finished; there is nothing to continue.');
    submit(CONTINUE);
  }

  // setQueue takes what waits for the session's agent, as the server says.
  function setQueue(v, queue) {
    v.queue = { items: queue?.items || [], paused: !!queue?.paused };
    // The session list says what waits, before its next refresh.
    const listed = v.ws === state.ws && state.sessions.find((s) => s.id === v.id);
    if (listed && ((listed.queued || 0) !== v.queue.items.length || !!listed.queue_paused !== v.queue.paused)) {
      listed.queued = v.queue.items.length;
      listed.queue_paused = v.queue.paused;
      cockpit.emit('session:sessions', state.sessions);
    }
    if (v === view()) {
      cockpit.emit('session:queue', v);
      cockpit.render();
    }
  }

  // followQueued follows a run the queue started because nothing was left to
  // wait for.
  function followQueued(v, run) {
    v.interrupted = false;
    state.outcome = null;
    if (v.fresh) history.replaceState(null, '', sessionHash(v.ws, v.id));
    attach(v, { id: run.id, started_at: run.started_at });
    refreshSessions();
  }

  // Esc Esc stops a run, and between runs edits the last prompt. Pressed
  // while a run stops, it opens the prompt as soon as the run has stopped.
  let escArmed = { at: 0, kind: '' }; // what the last lone Esc announced

  function escAction(v) {
    if (v.run) return v.run.phase === 'stopping' ? (cockpit.has('edit') ? 'edit-after-stop' : '') : 'stop';
    return cockpit.has('edit') && lastPrompt(v) && !runBlocked(v) ? 'edit' : '';
  }

  const ESC_HINT = {
    stop: 'Press Esc again to stop the run',
    edit: 'Press Esc again to edit your last prompt',
    'edit-after-stop': 'Press Esc again to edit the prompt once the run stops',
  };

  // armEsc handles an Esc outside of any field that uses it, and reports
  // whether it meant anything.
  function armEsc() {
    const v = view();
    if (!v) return false;
    const action = escAction(v);
    if (!action) return false;
    // The second Esc counts for what the first one announced: a run that ends
    // in between turns a stop into an edit, which needs another Esc Esc.
    const kind = action === 'stop' ? 'stop' : 'edit';
    if (escArmed.kind === kind && Date.now() - escArmed.at < 1600) {
      escArmed = { at: 0, kind: '' };
      if (action === 'stop') stop();
      else if (action === 'edit') {
        service('toast')?.dismiss?.('esc');
        service('edit')?.editLast?.();
      } else {
        v.editAfterStop = true;
        toast('The prompt opens for editing once the run has stopped', 'info', 'esc');
      }
      return true;
    }
    escArmed = { at: Date.now(), kind };
    toast(ESC_HINT[action], 'info', 'esc');
    return true;
  }

  cockpit.keys.register({ key: 'Escape', global: true, priority: 10, repeat: false, run: () => (armEsc() ? undefined : false) });

  // ---------------------------------------------------------------- session actions

  const sessionPath = (ws, id) => wsPath(ws, `/sessions/${encodeURIComponent(id)}`);

  function sessionInfo(ws, id) {
    return ws === state.ws ? state.sessions.find((s) => s.id === id) : undefined;
  }

  // busy reports a session some run holds, here or in another process.
  function busy(ws, id) {
    const v = view();
    return !!sessionInfo(ws, id)?.run_id || (v.ws === ws && v.id === id && (!!v.run || v.external));
  }

  async function rename(ws, id, title) {
    try {
      const res = await api(sessionPath(ws, id), { method: 'PATCH', body: { title } });
      const v = view();
      if (v.ws === ws && v.id === id) {
        v.title = res.title || firstPrompt(v);
        v.renamed = !!res.title;
        cockpit.render();
      }
      toast(title ? 'Session renamed' : 'Title reset to the first prompt');
    } catch (error) {
      toast(error.message, 'error');
    }
    refreshSessions();
  }

  async function pin(ws, id, pinned) {
    try {
      await api(sessionPath(ws, id), { method: 'PATCH', body: { pinned } });
      const v = view();
      if (v.ws === ws && v.id === id) v.pinned = pinned;
      toast(pinned ? 'Pinned to the top' : 'Unpinned');
    } catch (error) {
      toast(error.message, 'error');
    }
    cockpit.render();
    refreshSessions();
  }

  async function remove(ws, id) {
    try {
      await api(sessionPath(ws, id), { method: 'DELETE' });
    } catch (error) {
      toast(error.status === 409 ? 'Stop the run before deleting the session' : error.message, 'error');
      return;
    }
    service('composer')?.forget?.(ws, id);
    toast('Session deleted');
    if (view().ws === ws && view().id === id) {
      newSession({ push: false });
      history.replaceState(null, '', workspaceHash(state.ws));
    }
    refreshSessions();
  }

  // branch starts a session from the history before one prompt, with that
  // prompt in the composer, or duplicates the whole session.
  async function branch(ws, id, messageID) {
    const shown = view();
    const source = shown?.ws === ws && shown.id === id && messageID ? shown.entries.get(`input:${messageID}`) : null;
    let result;
    try {
      result = await api(`${sessionPath(ws, id)}/branch`, { method: 'POST', body: { message_id: messageID || '' } });
    } catch (error) {
      toast(error.status === 409 ? 'Stop the run before branching the session' : error.message, 'error');
      return;
    }
    if (ws !== state.ws) {
      toast('Branch created in the other workspace'); // the user moved on meanwhile
      return;
    }
    if (result.session_id) openSession(result.session_id);
    else newSession();
    if (result.prompt) {
      const composer = service('composer');
      composer?.set?.(result.prompt, { end: true });
      // The prompt's images come with it, from the session branched.
      if (source?.images?.length) service('images')?.load?.(source.images, (n) => service('images').sessionImageURL(ws, id, messageID, n));
      composer?.saveDraft?.();
    }
    toast(messageID ? 'Branched — edit the prompt and run it' : 'Session duplicated');
    refreshSessions();
  }

  function exportSession(ws, id) {
    const link = cockpit.h('a', { href: `${sessionPath(ws, id)}/export`, download: '' });
    document.body.append(link);
    link.click();
    link.remove();
  }

  // beginRename edits a title in place, where the session-list plugin can.
  function beginRename(ws, id, place = 'crumb') {
    if (cockpit.has('session-list')) return cockpit.use('session-list').rename(ws, id, place);
    const title = window.prompt('Title (empty: the first prompt)', view().title || firstPrompt(view()));
    if (title != null) rename(ws, id, title.trim());
    return undefined;
  }

  // menuItems lists what can be done to a session; place says where a
  // rename edits the title: in the rail or in the header.
  function menuItems(ws, id, place = 'crumb') {
    const v = view();
    const open = v.ws === ws && v.id === id;
    const pinned = open ? v.pinned : !!sessionInfo(ws, id)?.pinned;
    const held = busy(ws, id);
    const items = [
      { icon: '✎', label: 'Rename', hint: open ? 'F2' : '', run: () => beginRename(ws, id, place) },
      { icon: pinned ? '◇' : '◆', label: pinned ? 'Unpin' : 'Pin to top', run: () => pin(ws, id, !pinned) },
      { icon: '↳', label: 'Duplicate', disabled: held, run: () => branch(ws, id, '') },
      { icon: '↓', label: 'Export Markdown', run: () => exportSession(ws, id) },
      { icon: '#', label: 'Copy ID', run: () => copy(id, 'Session ID copied') },
      { icon: '×', label: 'Delete', danger: true, confirm: 'Click again to delete', disabled: held, run: () => remove(ws, id) },
    ];
    // Plugins add their own: contributions to "session.menu" with items(ws, id).
    for (const extra of cockpit.contributions('session.menu')) {
      const more = cockpit.safely(() => extra.items(ws, id, summary()));
      if (Array.isArray(more)) items.push(...more);
    }
    return items;
  }

  // sessionCommand runs an action on the session in view, once it is saved.
  function sessionCommand(action) {
    const v = view();
    if (v.fresh) return toast('The session is saved with its first run; run a prompt first.');
    return action(v);
  }

  const renameCommand = (arg = '') => sessionCommand((v) => {
    if (!arg) return beginRename(v.ws, v.id, 'crumb');
    return rename(v.ws, v.id, arg === '-' ? '' : arg);
  });
  const pinCommand = () => sessionCommand((v) => pin(v.ws, v.id, !v.pinned));
  const forkCommand = (arg = '') => sessionCommand((v) => {
    if (busy(v.ws, v.id)) return toast('Stop the run before branching the session.');
    if (!arg) return branch(v.ws, v.id, '');
    const { entry, error } = promptNumbered(v, arg);
    if (error) return toast(error, 'error');
    if (entry.state) return toast('That prompt never reached the runner.', 'error');
    return branch(v.ws, v.id, entry.id.replace(/^input:/, ''));
  });
  const exportCommand = () => sessionCommand((v) => exportSession(v.ws, v.id));
  const deleteCommand = () => sessionCommand((v) => {
    if (busy(v.ws, v.id)) return toast('Stop the run before deleting the session.');
    if (window.confirm('Delete this session and its history? This cannot be undone.')) remove(v.ws, v.id);
    return undefined;
  });
  const stopCommand = () => (view().run ? stop() : toast('Nothing is running.'));

  function copyAnswer() {
    const v = view();
    for (let i = v.order.length - 1; i >= 0; i--) {
      const entry = v.entries.get(v.order[i]);
      if (entry?.kind === 'assistant') return copy(entry.text, 'Answer copied');
    }
    return toast('There is no answer to copy yet.');
  }

  // resume opens the session an ID, an ID prefix or a title names.
  function resume(arg) {
    if (!arg) return toast('Name the session: its title, or the start of its ID.');
    const query = arg.toLowerCase();
    const exact = state.sessions.find((s) => s.id === arg);
    const found = exact ? [exact] : state.sessions.filter((s) => s.id.startsWith(query) || (s.title || '').toLowerCase().includes(query));
    if (!found.length) return toast(`No session matches "${arg}".`, 'error');
    if (found.length > 1) return toast(`${found.length} sessions match "${arg}"; pick one from the list.`, 'error');
    if (found[0].id !== view().id) openSession(found[0].id);
    return undefined;
  }

  // ---------------------------------------------------------------- workspaces

  // selectWorkspace makes a workspace this tab's; the caller shows a view in it.
  function selectWorkspace(id) {
    if (id === state.ws) return;
    service('menu')?.close?.();
    state.ws = id;
    prefs.set('workspace', id);
    // Nothing from the previous workspace's list may show under this one.
    state.sessions = [];
    state.sessionTicket++;
    cockpit.emit('session:workspace', id);
    cockpit.emit('session:sessions', state.sessions);
    refreshSessions();
    // The workspace's own plugins come, the last one's go.
    cockpit.host.setWorkspace(id);
    cockpit.render();
  }

  function switchWorkspace(id) {
    if (id === state.ws) return;
    selectWorkspace(id);
    newSession();
  }

  async function refreshWorkspaces() {
    const ticket = ++state.workspaceTicket;
    let list;
    try {
      list = await api('/api/workspaces');
    } catch {
      return;
    }
    if (ticket !== state.workspaceTicket) return;
    state.workspaces = list;
    if (!list.some((w) => w.id === state.ws) && list.length) {
      // Removed in another tab: fall back to the first workspace.
      toast('This workspace was removed from the list', 'error');
      switchWorkspace(list[0].id);
    }
    cockpit.emit('session:workspaces', list);
    cockpit.render();
  }

  async function removeWorkspace(id) {
    try {
      await api(`/api/workspaces/${encodeURIComponent(id)}`, { method: 'DELETE' });
    } catch (error) {
      toast(error.message, 'error');
      return;
    }
    state.workspaces = state.workspaces.filter((w) => w.id !== id);
    toast('Workspace removed from the list; its folder and sessions are untouched');
    if (state.ws === id && state.workspaces.length) switchWorkspace(state.workspaces[0].id);
    refreshWorkspaces();
  }

  // addWorkspace takes up a workspace just added and shows it.
  function addedWorkspace(added) {
    if (!state.workspaces.some((w) => w.id === added.id)) state.workspaces = [...state.workspaces, added];
    switchWorkspace(added.id);
    refreshWorkspaces();
  }

  async function refreshConfig() {
    try {
      state.config = await api('/api/config');
    } catch {
      return;
    }
    state.workspaces = state.config.workspaces || state.workspaces;
    cockpit.emit('session:config', state.config);
    cockpit.emit('session:workspaces', state.workspaces);
    cockpit.render();
  }

  // ---------------------------------------------------------------- what plugins see

  cockpit.provide('session', {
    get state() { return state; },
    CONTINUE, PHASE_LABEL,
    view, summary, entries: () => (view() ? view().order.map((id) => view().entries.get(id)) : []),
    sessionList: () => state.sessions.map((s) => ({ id: s.id, title: s.title, updated_at: s.updated_at, pinned: !!s.pinned, running: !!s.run_id })),
    workspaceList: () => state.workspaces.map((w) => ({ id: w.id, name: w.name, path: w.path, current: w.id === state.ws })),
    currentWorkspace, sessionHash, workspaceHash, sessionPath,
    runPhase, firstPrompt, lastPrompt, lastModel, previousModel, promptNumbered, runBlocked, busy, sessionInfo,
    nextModel, defaultModel,
    newSession, open: (id) => { if (id !== view()?.id) openSession(id); }, openSession, switchTo, ensureView, routeSessions,
    refreshSessions, refreshWorkspaces, refreshConfig, selectWorkspace, switchWorkspace, removeWorkspace, addedWorkspace,
    submit, prompt: (text) => submit(String(text)), compact, stop, stopCommand, continueRun,
    setQueue, followQueued, attach, upsert, touch, log, refreshTools,
    rename, pin, remove, branch, exportSession, menuItems, beginRename,
    renameCommand, pinCommand, forkCommand, exportCommand, deleteCommand, resume, copyAnswer,
  });

  // ---------------------------------------------------------------- commands, keys, palette

  const saved = (v) => !v.fresh;
  const commands = [
    {
      name: 'compact', args: '[focus]', help: 'Summarize the conversation to free context; the next prompt starts from the summary',
      shown: saved, run: (arg) => compact(arg, { fromComposer: true }),
    },
    { name: 'continue', help: 'Pick up the interrupted run', shown: (v) => v.interrupted && !v.running && !v.external, run: continueRun },
    { name: 'stop', help: 'Stop the run', shown: (v) => v.running, run: stopCommand },
    { name: 'new', help: 'Start a new session', run: () => newSession() },
    {
      name: 'resume', args: '<session>', help: 'Open a session by title or ID', shown: () => state.sessions.length > 0, run: resume,
      complete: (v) => state.sessions.map((s) => ({
        value: s.id, label: s.title || 'Untitled session',
        detail: [s.run_id ? 'running' : fmt.ago(s.updated_at), s.id.slice(0, 8), s.id === v.id ? 'open' : ''].filter(Boolean).join(' · '),
      })),
    },
    { name: 'rename', args: '[title]', help: 'Title the session; "-" brings back the first prompt', shown: saved, run: renameCommand },
    { name: 'pin', help: 'Pin the session to the top, or unpin it', shown: saved, run: pinCommand },
    { name: 'fork', aliases: ['branch'], args: '[n]', help: 'Branch before prompt n, or duplicate the session', shown: saved, run: forkCommand },
    { name: 'export', help: 'Download the session as Markdown', shown: saved, run: exportCommand },
    { name: 'delete', help: 'Delete the session and its history', shown: saved, run: deleteCommand },
    { name: 'copy', help: 'Copy the last answer', run: copyAnswer },
  ];
  // In the order the cockpit has always listed them, with the other
  // plugins' own between them.
  const ORDER = { compact: 10, continue: 20, stop: 30, new: 60, resume: 70, rename: 80, pin: 90, fork: 100, export: 110, delete: 120, copy: 180 };
  for (const command of commands) cockpit.commands.register({ ...command, order: ORDER[command.name] });

  // A prompt's Branch button, in the timeline.
  cockpit.contribute('timeline.action', {
    id: 'branch', kinds: ['user'], order: 40, label: 'Branch', title: 'New session with the history before this prompt, and the prompt ready to edit',
    shown: (entry, ctx) => !entry.state && !ctx.view.fresh && !ctx.view.run && !ctx.view.external,
    run: (entry, ctx) => branch(ctx.view.ws, ctx.view.id, entry.id.replace(/^input:/, '')),
  });

  cockpit.keys.register({ key: 'n', run: () => newSession() });
  cockpit.contribute('help.keys', { keys: ['N'], text: 'New session', order: 100 });
  cockpit.contribute('help.keys', { keys: ['Esc', 'Esc'], text: 'Stop the run · edit the last prompt', order: 50 });

  cockpit.contribute('palette.provider', {
    id: 'session', order: 100,
    items(s) {
      const v = view();
      if (!v) return [];
      return [
        { group: 'Actions', icon: '+', label: 'New session', hint: 'N', order: 100, run: () => newSession() },
        v.run && { group: 'Actions', icon: '■', label: 'Stop the run', hint: 'Esc Esc', order: 101, run: stop },
        v.interrupted && !v.run && !v.external && { group: 'Session', icon: '▶', label: 'Continue the interrupted run', order: 300, run: () => submit(CONTINUE) },
        !v.fresh && !runBlocked(v) && {
          group: 'Session', icon: '⇲', label: 'Compact the context', hint: '/compact', order: 302,
          detail: 'Summarize the conversation to free context', run: () => compact(),
        },
        !v.fresh && { group: 'Session', icon: '✎', label: 'Rename session', hint: 'F2', order: 303, run: () => beginRename(v.ws, v.id, 'crumb') },
        !v.fresh && { group: 'Session', icon: v.pinned ? '◇' : '◆', label: v.pinned ? 'Unpin session' : 'Pin session', order: 304, run: () => pin(v.ws, v.id, !v.pinned) },
        !v.fresh && !busy(v.ws, v.id) && { group: 'Session', icon: '↳', label: 'Duplicate session', order: 305, run: () => branch(v.ws, v.id, '') },
        !v.fresh && { group: 'Session', icon: '↓', label: 'Export as Markdown', order: 306, run: () => exportSession(v.ws, v.id) },
        !v.fresh && { group: 'Session', icon: '#', label: 'Copy session ID', order: 307, run: () => copy(v.id, 'Session ID copied') },
        !v.fresh && !busy(v.ws, v.id) && {
          group: 'Session', icon: '×', label: 'Delete session', order: 308,
          run: () => { if (window.confirm('Delete this session and its history? This cannot be undone.')) remove(v.ws, v.id); },
        },
        ...state.sessions.map((item) => ({
          group: 'Sessions', icon: item.run_id ? '■' : '·', label: item.title || 'Untitled session', hint: item.run_id ? 'running' : fmt.ago(item.updated_at),
          detail: item.id, order: 900, run: () => { if (item.id !== view().id) openSession(item.id); },
        })),
      ].filter(Boolean);
    },
  });

  // ---------------------------------------------------------------- clock, visibility, failures

  // The run's clock and a finished run's outcome, which shows for a while.
  cockpit.interval(() => {
    const v = view();
    if (!v) return;
    if (state.outcome && Date.now() - state.outcome.at >= 5000) {
      state.outcome = null;
      cockpit.render();
    } else if (v.run) {
      cockpit.render();
    }
  }, 1000);

  cockpit.listen(document, 'visibilitychange', () => {
    if (document.visibilityState === 'visible') {
      state.unseenOutcome = null;
      refreshSessions();
      cockpit.render();
    }
    schedulePoll();
  });

  // A plugin that fails says so in the runner log of the session in view.
  cockpit.on('plugin-error', ({ text }) => log(text));
  cockpit.on('online', () => cockpit.render());

  // ---------------------------------------------------------------- start, or pick up where the last version was

  async function boot() {
    state.booted = true;
    try {
      state.config = await api('/api/config');
    } catch {
      state.config = null;
    }
    if (state.config?.settings_error) toast(`Connection settings: ${state.config.settings_error}`, 'error');
    if (state.config) {
      state.workspaces = state.config.workspaces || [];
      // The tab opens where it was last, unless the address says otherwise.
      const last = prefs.get('workspace', null);
      state.ws = state.workspaces.some((w) => w.id === last) ? last : state.config.workspace_id || state.workspaces[0]?.id || null;
    }
    state.configured = true;
    cockpit.emit('session:config', state.config);
    cockpit.emit('session:workspaces', state.workspaces);
    cockpit.emit('session:workspace', state.ws);
    if (state.ws) cockpit.host.setWorkspace(state.ws);
    // Until every plugin is in, the kernel routes once they are.
    if (cockpit.host.booted()) cockpit.route();
    ensureView();
    await refreshSessions();
    schedulePoll();
  }

  function pickUp() {
    const v = view();
    // The version before closed the run's stream: this one follows it.
    if (v?.run?.id && !v.run.source) {
      attach(v, {
        id: v.run.id, started_at: new Date(v.run.started).toISOString(), stopping: v.run.phase === 'stopping', compact: v.run.compact,
        activity: v.run.activity,
      });
    }
    if (v) {
      cockpit.emit('session:view', v, { reload: true, previous: v });
      touch(v);
    }
    cockpit.emit('session:sessions', state.sessions);
    schedulePoll();
  }

  cockpit.onDispose(() => {
    detach(view());
    clearTimeout(sessionsPoll);
  });

  if (!state.booted) boot();
  else queueMicrotask(pickUp);
}
