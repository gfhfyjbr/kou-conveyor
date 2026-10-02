// update: whether a newer CLIProxyAPI is out than the one the server is
// built with, and taking it up. Once the page opens, the Accounts view asks
// the server (GET /api/gateway/update), which asks the module proxies at
// most once an hour, and offers a newer release in a dialog. A server that
// builds itself from its checkout takes the release up (POST
// /api/gateway/update): it moves go.mod and go.sum to it once its programs
// build with it, then builds itself anew and restarts, as it does whenever
// its Go code changes, and the dialog follows it there. Otherwise the dialog
// says what to run. A release skipped is not offered again, and Later waits
// a day; the badge beside the version offers it all the same.
import { fmt } from '/kernel/dom.js';
import { h, sync } from './gateway.js';

const POLL = 1000;
const LATER = 24 * 60 * 60 * 1000;
const SKIP = 'gateway-update-skip'; // the release not to offer
const SNOOZE = 'gateway-update-later'; // { version, until }
// What the server says of its own build, once go.mod moved.
const BUILD = {
  building: 'Building the server anew',
  waiting: 'Built — the server restarts once the agents at work finish',
  restarting: 'Restarting the server',
};

const capital = (text) => text.charAt(0).toUpperCase() + text.slice(1);

// createUpdates makes the dialog and the badge; ctx gives api, prefs,
// toast(text, kind, key), copy(text, label), overlaysOpen() and
// closeOverlays().
export function createUpdates(ctx) {
  const { api, prefs, toast, copy } = ctx;
  const ui = {
    status: null, // the server's latest answer
    phase: 'idle', // checking, offer, manual, working, building, done, failed, current, error
    target: '', // the release the server moves to
    build: null, // what the server said last of its build, once go.mod moved
    error: '', // why the check, or the server's build, failed
    output: '',
    notice: '', // why the server refused to update
    missing: false, // the server runs a build from before updates
    busy: false,
    timer: 0,
    dead: false,
  };

  const title = h('span', { class: 'label', id: 'gw-update-title', text: 'CLIProxyAPI update' });
  const body = h('div', { class: 'settings-body', id: 'gw-update-body' });
  const foot = h('footer', { id: 'gw-update-foot' });
  const dialog = h('div', { class: 'settings gw-update ticks', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'gw-update-title', tabindex: '-1' },
    h('header', null, title, h('button', { class: 'icon', id: 'gw-update-close', type: 'button', 'aria-label': 'Close', onclick: () => close() }, '×')),
    body, foot);
  const overlay = h('div', { class: 'overlay', id: 'gw-update', hidden: true, onmousedown: (event) => { if (event.target === overlay) close(); } }, dialog);
  const badge = h('button', { class: 'gw-update-badge', id: 'gw-update-badge', type: 'button', hidden: true, onclick: () => open() });

  const name = () => ui.status?.name || 'CLIProxyAPI';
  const latest = () => ui.status?.latest?.version || '';

  // ---------------------------------------------------------------- the server's answers

  // phaseOf is what the dialog shows of an answer.
  function phaseOf(s) {
    const job = s.update || {};
    if (job.state === 'updating') return 'working';
    if (job.state === 'updated' && s.running !== job.version) return 'building';
    if (s.available && s.latest) return s.can_update ? 'offer' : 'manual';
    return s.latest || !s.error ? 'current' : 'error';
  }

  // apply takes an answer of the server's in.
  function apply(s) {
    const was = ui.phase;
    const job = s.update || {};
    ui.status = s;
    if (job.state === 'updating' || job.state === 'updated') ui.target = job.version;
    // What the server's build of itself did last, unless an event said
    // something later.
    if (s.build && !(Date.parse(ui.build?.at) > Date.parse(s.build.at))) ui.build = s.build;
    let next = phaseOf(s);
    // Once go.mod moved, the server builds itself anew and restarts: the
    // update is done when the server runs the release.
    if (['building', 'failed'].includes(was) && next !== 'working' && ui.target) {
      if (s.running === ui.target) next = 'done';
      else if (s.required === ui.target) next = 'building';
    }
    if (overlay.hidden && was === 'working' && job.state === 'failed') toast(`The ${name()} update failed: ${job.message}`, 'error', 'gw-update');
    if (overlay.hidden && was !== 'done' && next === 'done') toast(`The server runs ${name()} ${ui.target} now`, 'info', 'gw-update');
    ui.phase = next;
    // The server's build with the release failed, perhaps before this page
    // heard that go.mod moved.
    if (next === 'building' && ui.build?.state === 'failed' && !(Date.parse(ui.build.at) < Date.parse(job.at))) failBuild(ui.build);
    poll();
    render();
  }

  // failBuild: go.mod moved, but the server's build with it failed.
  function failBuild(status) {
    ui.phase = 'failed';
    ui.error = `go.mod requires ${name()} ${ui.target} now, but ${status.message || 'the build failed'}.`;
    ui.output = status.output || '';
  }

  // poll asks again in a moment while the update runs.
  function poll() {
    clearTimeout(ui.timer);
    ui.timer = 0;
    if (ui.phase !== 'working' || ui.dead) return;
    ui.timer = setTimeout(async () => {
      ui.timer = 0;
      try {
        apply(await api('/api/gateway/update'));
      } catch {
        poll();
      }
    }, POLL);
  }

  // offered says whether the dialog opens by itself for an answer: for a
  // newer release neither skipped nor put off.
  function offered(s) {
    const version = s.latest?.version;
    if (!s.available || !version || !['offer', 'manual'].includes(ui.phase)) return false;
    if (prefs.get(SKIP, '') === version) return false;
    const later = prefs.get(SNOOZE, null);
    return !(later?.version === version && later.until > Date.now());
  }

  // check asks the server whether a newer release is out; with popup, the
  // dialog offers it, unless another dialog is open.
  async function check({ popup = false, refresh = false } = {}) {
    let s;
    try {
      s = await api(`/api/gateway/update${refresh ? '?refresh=1' : ''}`);
    } catch (error) {
      // A server built before updates has no such request; the build it
      // restarts with will.
      ui.missing = error?.status === 404;
      return null;
    }
    if (ui.dead) return null;
    ui.missing = false;
    apply(s);
    if (popup && overlay.hidden && offered(s) && !ctx.overlaysOpen().length) show();
    return s;
  }

  // ---------------------------------------------------------------- the dialog

  // open shows the dialog; refresh has the server ask the module proxies
  // anew first.
  async function open({ refresh = false } = {}) {
    ctx.closeOverlays?.();
    ui.notice = '';
    if (!ui.status || refresh && !['working', 'building'].includes(ui.phase)) ui.phase = 'checking';
    show();
    if (ui.phase !== 'checking') return;
    const s = await check({ refresh: true });
    if (!s && !ui.dead) {
      ui.phase = 'error';
      ui.error = ui.missing
        ? 'The server runs a build from before it could check. It checks once it restarts with a newer one.'
        : 'The server could not be reached.';
      render();
    }
  }

  function show() {
    overlay.hidden = false;
    render();
    dialog.focus({ preventScroll: true });
  }

  function close() {
    if (overlay.hidden) return;
    overlay.hidden = true;
    // What is over is not shown again; a build that failed is, until the
    // server builds again.
    if (['checking', 'done', 'error'].includes(ui.phase)) ui.phase = ui.status ? phaseOf(ui.status) : 'idle';
    render();
  }

  function skip() {
    const version = latest();
    prefs.set(SKIP, version);
    close();
    toast(`${name()} ${version} is not offered again; the badge beside the version still does`, 'info', 'gw-update');
  }

  function later() {
    prefs.set(SNOOZE, { version: latest(), until: Date.now() + LATER });
    close();
  }

  async function start() {
    const version = latest();
    if (!version || ui.busy) return;
    ui.busy = true;
    ui.notice = '';
    ui.build = null;
    render();
    try {
      ui.target = version;
      apply(await api('/api/gateway/update', { method: 'POST', body: { version } }));
    } catch (error) {
      if (error?.body?.status) apply(error.body.status);
      ui.notice = error.message;
    } finally {
      ui.busy = false;
      render();
    }
  }

  // serverBuild follows the server's builds of itself (the 'server' event):
  // once go.mod moved, it builds itself with the release — and again after
  // a build that failed, once its code changes.
  function serverBuild(status) {
    if (!status) return;
    ui.build = status;
    if (!['building', 'failed'].includes(ui.phase)) return;
    if (status.state === 'failed') failBuild(status);
    else ui.phase = 'building';
    render();
  }

  // restarted: the server runs a new build — with the release, taken up on
  // this page or another, or one that knows updates at last.
  function restarted() {
    if (['building', 'failed'].includes(ui.phase) || ui.status?.available || ui.missing) check({ popup: ui.missing });
  }

  // ---------------------------------------------------------------- rendering

  const note = (row, text) => h('p', { class: 'settings-note', data: { row }, text });
  const button = (label, run, cls = 'act') => h('button', { class: cls, type: 'button', onclick: run }, label);
  const stateLine = (text) => h('div', { class: 'state-line gw-update-state', data: { row: 'state' } },
    h('span', { class: 'meter', 'aria-hidden': 'true' }, Array.from({ length: 8 }, () => h('i'))), h('span', { text }));
  const failure = (heading, message, output) => [
    h('div', { class: 'load-error', data: { row: 'error' } }, h('b', { text: heading }), h('span', { text: message || '' })),
    output ? h('pre', { class: 'gw-update-output', data: { row: 'output' }, text: output }) : null,
  ];
  const done = (heading, text) => h('div', { class: 'signin-done', data: { row: 'done' } },
    h('span', { class: 'done-mark', 'aria-hidden': 'true', text: '✓' }),
    h('div', null, h('b', { text: heading }), text ? h('p', { class: 'settings-note', text }) : null));

  function versions(from, to) {
    return h('div', { class: 'gw-update-versions', data: { row: 'versions' } },
      h('span', { class: 'from', text: from || '?' }), h('span', { class: 'arrow', 'aria-hidden': 'true', text: '→' }), h('b', { class: 'to', text: to }));
  }

  function facts(s) {
    const parts = [];
    if (s.behind) parts.push(s.behind === 1 ? 'the next release' : `${s.behind} releases ahead`);
    const at = s.latest?.time ? new Date(s.latest.time) : null;
    if (at && !Number.isNaN(at.getTime())) parts.push(`released ${at.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })}`);
    return h('p', { class: 'gw-update-facts', data: { row: 'facts' } }, parts.map((part) => `${part} · `), h('span', { class: 'module', text: s.module }));
  }

  function major(s) {
    const m = s?.major;
    return m ? h('p', { class: 'gw-update-major', data: { row: 'major' }, text: `${s.name} ${m.version} is out as well: a new major version, which is another Go module (${m.path}) with an API of its own. Moving to it takes changes to the cockpit's code, so it is not offered here.` }) : null;
  }

  // link leads to what changed from the version required to the release.
  function link(s) {
    const href = ['offer', 'manual'].includes(ui.phase) ? s?.changes || s?.latest?.url : '';
    return h('span', { class: 'settings-path', data: { row: 'path' } },
      href ? h('a', { class: 'gw-update-link', href, target: '_blank', rel: 'noopener noreferrer', text: s.changes ? 'What changed ↗' : 'Release notes ↗' }) : null);
  }

  function render() {
    renderBadge();
    if (overlay.hidden) return;
    const s = ui.status;
    let nodes = [];
    let buttons = [];
    switch (ui.phase) {
      case 'checking':
        nodes = [stateLine(`Asking for the latest ${name()}`)];
        buttons = [button('Close', close)];
        break;
      case 'offer':
      case 'manual': {
        const job = s.update || {};
        const offer = ui.phase === 'offer';
        nodes = [
          versions(s.required || s.running, s.latest.version),
          facts(s),
          job.state === 'failed' && job.version === s.latest.version ? failure('The last update failed', job.message, job.output) : null,
          offer
            ? note('what', `The server moves go.mod and go.sum to ${s.latest.version} once its programs build with it — then builds itself anew and restarts with it as soon as no agent is at work. Pages reconnect by themselves.`)
            : note('what', `It cannot be taken up from here: ${s.reason}.`),
          !offer && s.command ? h('div', { class: 'gw-update-command', data: { row: 'command' } },
            h('code', { text: s.command }), button('Copy', () => copy(s.command, 'Command copied'))) : null,
          !offer ? note('how', `Run it in the checkout${s.checkout ? ` (${s.checkout})` : ' of kou-conveyor'}, then build the programs again: make build.`) : null,
          s.running && s.required && s.running !== s.required ? note('running', `The server runs ${s.running} until it restarts with a build of go.mod's ${s.required}.`) : null,
          major(s),
          ui.notice ? h('p', { class: 'settings-status', data: { kind: 'error', row: 'notice' }, text: ui.notice }) : null,
        ];
        buttons = [button('Skip this version', skip), button('Later', later),
          offer
            ? h('button', { class: 'primary', type: 'button', disabled: ui.busy, onclick: start }, ui.busy ? 'Starting…' : `Update to ${s.latest.version}`)
            : button('Close', close, 'primary')];
        break;
      }
      case 'working':
        nodes = [versions(s?.running || s?.required, ui.target),
          stateLine(capital(s?.update?.message || `updating ${name()}`)),
          note('what', 'go.mod and go.sum change only once the programs build with the new version; until then the server goes on as it is.')];
        buttons = [button('Hide', close)];
        break;
      case 'building':
        nodes = [versions(s?.running, ui.target),
          stateLine(BUILD[ui.build?.state] || BUILD.building),
          note('what', `go.mod requires ${name()} ${ui.target} now. The server builds itself with it, and restarts once no agent is at work; this page reconnects by itself.`),
          s?.update?.output ? h('pre', { class: 'gw-update-output', data: { row: 'output' }, text: s.update.output }) : null];
        buttons = [button('Hide', close)];
        break;
      case 'done':
        nodes = [done(`The server runs ${name()} ${ui.target}`, 'It restarted with the new build.')];
        buttons = [button('Done', close, 'primary')];
        break;
      case 'failed':
        nodes = failure('The server did not restart with it', ui.error, ui.output);
        buttons = [button('Close', close, 'primary')];
        break;
      case 'error':
        nodes = failure(`Could not check for a newer ${name()}`, s?.error || ui.error);
        buttons = [button('Check again', () => open({ refresh: true })), button('Close', close, 'primary')];
        break;
      default: {
        // With a newer major version out, it is the latest of its own.
        const current = s?.required || s?.running || '';
        const line = s?.major && current ? ` ${current.split('.')[0]}` : '';
        nodes = [done(`${name()} ${current} is the latest${line} release`, s?.checked_at ? `${s.module}, checked ${fmt.stamp(s.checked_at)}` : ''), major(s)];
        buttons = [button('Check again', () => open({ refresh: true })), button('Close', close, 'primary')];
      }
    }
    sync(body, nodes.flat());
    sync(foot, [link(s), ...buttons]);
  }

  function renderBadge() {
    const s = ui.status;
    let text = '';
    let tip = '';
    let major = false;
    if (['working', 'building', 'failed'].includes(ui.phase)) {
      text = `↑ ${ui.target}`;
      tip = ui.phase === 'failed' ? `The server did not restart with ${name()} ${ui.target}: its build failed` : `Updating ${name()} to ${ui.target}`;
    } else if (s?.available && s.latest) {
      text = `↑ ${s.latest.version}`;
      tip = `${name()} ${s.latest.version} is out: what the update does`;
    } else if (s?.major) {
      // A newer major version is another module, which takes changes to the
      // cockpit's code: the badge tells of it, and the dialog says why.
      text = `↑ ${s.major.version}`;
      tip = `${name()} ${s.major.version} is out: a new major version, which takes changes to the cockpit's code`;
      major = true;
    }
    badge.hidden = !text;
    if (badge.textContent !== text) badge.textContent = text;
    badge.title = tip;
    badge.dataset.phase = ui.phase;
    badge.dataset.major = String(major);
  }

  return {
    overlay, badge, check, open, close, serverBuild, restarted,
    isOpen: () => !overlay.hidden,
    // available is the release on offer, if one is.
    available: () => (ui.status?.available ? latest() : ''),
    // resume: the plugin loaded anew, on a page that asked already.
    resume: () => check(),
    destroy() {
      ui.dead = true;
      clearTimeout(ui.timer);
    },
  };
}
