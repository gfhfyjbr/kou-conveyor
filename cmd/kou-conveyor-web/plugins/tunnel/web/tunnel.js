// tunnel: the cockpit carried through a relay on another host — to the
// iPhone app and to browsers elsewhere — while it goes on here as it was
// (kou-conveyor-web tunnel). Beside the mark (rail.head), a rectangle with a
// planet says "tunnelling" while it is on. /tunnel starts it: when the relay
// answers, the cockpit connects to it at once; when there is none yet, or
// it does not answer, the agent is asked, in a session of its own, to set it
// up — to install kou-conveyor-relay on a host it asks the user for — and the
// cockpit connects once it is done. Its dialog says how it goes, and holds
// the code that pairs the iPhone app. It provides the tunnel service:
// status(), open(), close(), start(), stop(), setup(), refresh().
const PLANET = '<svg viewBox="0 0 20 20" aria-hidden="true"><ellipse cx="10" cy="10" rx="9" ry="3.1" transform="rotate(-24 10 10)" fill="none" stroke="currentColor" stroke-width="1.6"/><circle class="tunnel-planet" cx="10" cy="10" r="4.7" stroke="currentColor" stroke-width="1.6"/><path d="M1 10A9 3.1 0 0 0 19 10" transform="rotate(-24 10 10)" fill="none" stroke="currentColor" stroke-width="1.6"/></svg>';

const ON = new Set(['setup', 'connecting', 'up', 'retrying']);
const HEADLINE = {
  off: 'Off', unavailable: 'Unavailable', setup: 'Waiting for a relay', connecting: 'Connecting', up: 'Tunnelling', retrying: 'Reconnecting',
};

export default function activate(cockpit) {
  const { h, svg } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const toast = (text, kind = 'info') => cockpit.toast(text, kind, 'tunnel');
  let current = { state: 'off' };
  let disposed = false;

  function host(url) {
    try {
      return new URL(url).host;
    } catch {
      return url || '';
    }
  }

  const fingerprint = (fp) => (fp ? `${fp.slice(0, 8)}…${fp.slice(-8)}` : '');

  // ---------------------------------------------------------------- the rail

  const chip = h('button', { class: 'tunnel-chip', id: 'tunnel-chip', type: 'button', hidden: true, onclick: () => open() },
    svg(PLANET), h('span', { class: 'tunnel-word', text: 'tunnelling' }));
  cockpit.ui.mount('rail.head', { id: 'tunnel', order: 10, node: chip });

  // fit lets the word go when the rail is too narrow for it beside the mark;
  // the rail is followed as it is resized.
  const resize = typeof ResizeObserver === 'function' ? new ResizeObserver(() => fit()) : null;
  let followed = null;
  function fit() {
    const head = chip.parentElement;
    if (chip.hidden || !head) return;
    if (resize && followed !== head) {
      resize.disconnect();
      resize.observe(head);
      followed = head;
    }
    chip.classList.remove('compact');
    const room = head.getBoundingClientRect().right - (parseFloat(getComputedStyle(head).paddingRight) || 0);
    if (chip.getBoundingClientRect().right > room + 0.5) chip.classList.add('compact');
  }

  function describe(status) {
    const where = host(status.url);
    switch (status.state) {
      case 'up': return `Tunnelling through ${where}: the iPhone app and browsers elsewhere reach this cockpit`;
      case 'connecting': return `Tunnel: connecting to ${where}…`;
      case 'retrying': return `Tunnel: ${status.error || 'the relay is not reached'}; trying again`;
      case 'setup': return 'Tunnel: waiting for a relay to be set up';
      default: return 'Tunnel: off';
    }
  }

  // ---------------------------------------------------------------- the dialog

  const say = h('div', { class: 'settings-status', id: 'tunnel-status', role: 'status', 'aria-live': 'polite' });
  const headline = h('b', { class: 'tunnel-headline' });
  const detail = h('p', { class: 'settings-note', id: 'tunnel-detail' });
  const facts = h('dl', { class: 'tunnel-facts', id: 'tunnel-facts' });
  const remote = h('p', { class: 'tunnel-remote', hidden: true, text: 'This page comes through the tunnel itself: stopping it cuts this page off.' });
  const qr = h('img', { class: 'tunnel-qr-image', alt: 'The code that pairs the iPhone app', width: 168, height: 168 });
  const reveal = h('button', { class: 'act strong', id: 'tunnel-reveal', type: 'button', onclick: () => showCode(!ui.code) }, 'Show code');
  const copyApp = h('button', { class: 'act', type: 'button', onclick: () => current.pair_link && cockpit.copy(current.pair_link, 'The link that pairs the iPhone app') }, 'Copy app link');
  const copyBrowser = h('button', { class: 'act', type: 'button', onclick: () => current.browser_link && cockpit.copy(current.browser_link, 'The link that opens the cockpit through the relay') }, 'Copy browser link');
  const pair = h('section', { class: 'tunnel-pair', hidden: true, data: { shown: 'false' } },
    h('button', { class: 'tunnel-qr', type: 'button', title: 'Show or hide the pairing code', 'aria-label': 'Show or hide the pairing code', onclick: () => showCode(!ui.code) },
      h('span', { class: 'tunnel-qr-veil' }, svg(PLANET)), qr),
    h('div', { class: 'tunnel-pair-text' },
      h('span', { class: 'label', text: 'Pair the iPhone' }),
      h('p', { class: 'settings-note', text: "Scan the code with the iPhone's camera, or open the app link on the phone: the kou-conveyor app pairs with the relay. The code holds the relay's token: keep it to yourself." }),
      h('div', { class: 'tunnel-copies' }, reveal, copyApp, copyBrowser)));
  const where = h('span', { class: 'settings-path', id: 'tunnel-path' });
  const setupButton = h('button', { class: 'act', id: 'tunnel-setup', type: 'button', onclick: () => setup() }, 'Set up with the agent');
  const checkButton = h('button', { class: 'act', id: 'tunnel-check', type: 'button', onclick: () => check() }, 'Check');
  const toggle = h('button', { class: 'primary', id: 'tunnel-toggle', type: 'button', onclick: () => (current.wanted ? stop() : start()) }, 'Start');
  const dialog = h('div', { class: 'settings ticks tunnel', id: 'tunnel-dialog', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'tunnel-title' },
    h('header', null, h('span', { class: 'label', id: 'tunnel-title', text: 'Tunnel' }),
      h('button', { class: 'icon', id: 'tunnel-close', type: 'button', 'aria-label': 'Close', onclick: () => close() }, '×')),
    h('div', { class: 'settings-body' },
      h('div', { class: 'tunnel-state' }, h('span', { class: 'tunnel-orb' }, svg(PLANET)), h('div', null, headline, detail)),
      facts, remote, pair, say),
    h('footer', null, where, setupButton, checkButton, toggle));
  const overlay = h('div', { class: 'overlay', id: 'tunnel', hidden: true, onmousedown: (event) => { if (event.target === overlay) close(); } }, dialog);
  cockpit.ui.mount('overlays', { id: 'tunnel', order: 55, node: overlay });
  cockpit.contribute('overlay', { id: 'tunnel', order: 55, modal: true, isOpen: () => !overlay.hidden, close: () => close() });

  const ui = { code: false, busy: false };

  function note(text, kind = '') {
    say.textContent = text;
    say.dataset.kind = kind;
  }

  function open() {
    service('overlays')?.closeTop?.();
    overlay.hidden = false;
    note('');
    render();
    refresh();
    schedule();
    toggle.focus();
  }

  function close() {
    overlay.hidden = true;
    showCode(false);
    schedule();
  }

  // showCode shows the pairing code, or hides it: it holds the token, and
  // screens are shared.
  function showCode(show) {
    ui.code = show && Boolean(current.configured);
    pair.dataset.shown = ui.code ? 'true' : 'false';
    reveal.textContent = ui.code ? 'Hide code' : 'Show code';
    if (ui.code) qr.src = `/api/tunnel/qr?t=${Date.now()}`;
    else qr.removeAttribute('src');
  }

  function fact(label, value, title) {
    return [h('dt', { class: 'label', text: label }), h('dd', { text: value, title: title || value })];
  }

  function render() {
    const status = current;
    overlay.dataset.state = status.state;
    headline.textContent = HEADLINE[status.state] || status.state;
    let text = '';
    switch (status.state) {
      case 'up': text = `Through ${status.url}: the iPhone app and browsers elsewhere reach this cockpit, which goes on here as it was.`; break;
      case 'connecting': text = `To ${status.url}…`; break;
      case 'retrying': text = `${status.error || 'The relay is not reached'}.${status.retry_at ? ` Again at ${new Date(status.retry_at).toLocaleTimeString()}.` : ''}`; break;
      case 'setup': text = 'No relay is set up yet. The agent sets one up: it installs kou-conveyor-relay on a host you name, over ssh; the cockpit connects once it is done.'; break;
      case 'unavailable': text = 'There is nowhere to keep tunnel.json: set KOU_CONVEYOR_TUNNEL_CONFIG.'; break;
      default: text = status.configured
        ? `The relay at ${status.url} is set up. Start carries the cockpit through it.`
        : 'Carry this cockpit to the iPhone app and to browsers elsewhere, through a relay on a host of yours: the agent installs it there.';
    }
    detail.textContent = text;
    const rows = [];
    if (status.url) rows.push(...fact('Relay', status.url));
    if (status.host) rows.push(...fact('Host', status.host));
    if (status.fingerprint) rows.push(...fact('Certificate', `sha-256 ${fingerprint(status.fingerprint)}`, status.fingerprint));
    facts.replaceChildren(...rows);
    facts.hidden = rows.length === 0;
    remote.hidden = !status.remote;
    pair.hidden = !status.configured;
    if (!status.configured && ui.code) showCode(false);
    where.textContent = status.config || '';
    toggle.textContent = status.wanted ? 'Stop' : 'Start';
    toggle.disabled = ui.busy || status.state === 'unavailable';
    checkButton.disabled = ui.busy || !status.configured;
    checkButton.hidden = !status.configured;
    setupButton.disabled = ui.busy || status.state === 'unavailable';
    setupButton.textContent = status.configured ? 'Set up again with the agent' : 'Set up with the agent';
  }

  function apply(status) {
    if (!status || typeof status !== 'object') return;
    const was = current.state;
    current = status;
    const on = ON.has(status.state);
    const shown = chip.hidden && on;
    chip.hidden = !on;
    chip.dataset.state = status.state;
    if (shown) fit();
    const text = describe(status);
    chip.title = `${text}. Click: the tunnel.`;
    chip.setAttribute('aria-label', text);
    if (!overlay.hidden) render();
    if (was !== status.state && status.state === 'up') {
      // It lights up once as it comes up.
      chip.classList.remove('lit');
      void chip.offsetWidth;
      chip.classList.add('lit');
      if (was !== 'off') toast(`Tunnel up · ${host(status.url)}`);
    }
  }

  // ---------------------------------------------------------------- following the server

  let timer = 0;
  let fetching = false;

  async function refresh() {
    if (fetching) return;
    fetching = true;
    try {
      apply(await cockpit.api('/api/tunnel'));
    } catch {
      // The server restarts, or is gone: the next look says.
    } finally {
      fetching = false;
    }
  }

  // schedule looks again: often while the dialog is open or the tunnel is
  // on its way, seldom else, and not while the page is hidden.
  function schedule() {
    clearTimeout(timer);
    if (disposed) return;
    const busy = !overlay.hidden || ['setup', 'connecting', 'retrying'].includes(current.state);
    timer = setTimeout(async () => {
      if (!document.hidden) await refresh();
      schedule();
    }, busy ? 1500 : 6000);
  }

  const onVisible = () => { if (!document.hidden) refresh(); };
  document.addEventListener('visibilitychange', onVisible);
  cockpit.onDispose(() => {
    disposed = true;
    clearTimeout(timer);
    resize?.disconnect();
    document.removeEventListener('visibilitychange', onVisible);
  });
  refresh().then(schedule);

  // ---------------------------------------------------------------- the actions

  async function act(action) {
    ui.busy = true;
    render();
    try {
      const answer = await cockpit.api('/api/tunnel', { method: 'POST', body: { action } });
      apply(answer.tunnel);
      return answer;
    } catch (error) {
      note(error.message, 'error');
      toast(`Tunnel: ${error.message}`, 'error');
      return null;
    } finally {
      ui.busy = false;
      render();
      schedule();
    }
  }

  // start has the cockpit carried through the relay: at once when the relay
  // answers; else the agent sets it up, and the cockpit connects once it has.
  async function start() {
    note('Checking the relay…');
    const answer = await act('start');
    if (!answer) return;
    if (answer.check?.ok) {
      note(`The relay answers${answer.check.version ? ` (kou-conveyor-relay ${answer.check.version})` : ''}; the cockpit connects to it.`, 'ok');
      if (overlay.hidden) open();
      return;
    }
    setup(answer.check, answer.tunnel);
  }

  async function stop() {
    if (current.remote && !confirm('This page comes through the tunnel: stopping it cuts this page off. Stop it?')) return;
    const answer = await act('stop');
    if (answer) {
      note('Stopped: the cockpit lets the relay go, and stays off when it starts again.', '');
      toast('Tunnel off');
    }
  }

  async function check() {
    note('Checking the relay…');
    const answer = await act('check');
    const result = answer?.check;
    if (!result) return;
    if (result.ok) {
      note(`The relay answers${result.version ? ` (kou-conveyor-relay ${result.version})` : ''}${result.agent ? '; a cockpit is connected to it' : '; no cockpit is connected to it yet'}.`, 'ok');
    } else {
      note(`${result.error}${result.code ? ` [${result.code}]` : ''}`, 'error');
    }
  }

  // setup hands the setting up of the relay to the agent, in a session of
  // its own: the cockpit is told to want the tunnel first, so it connects as
  // soon as the agent is done.
  async function setup(result = null, status = null) {
    if (!status) {
      const answer = await act('start');
      if (!answer) return;
      result = answer.check;
      status = answer.tunnel;
    }
    const text = setupPrompt(result, status || current);
    const v = session.summary?.();
    const fresh = v && v.fresh && !v.entries && !v.running;
    if (!fresh) session.newSession?.();
    close();
    cockpit.prompt(text);
    toast(result?.code === 'not_configured' || !result
      ? 'Tunnel: the agent sets the relay up; it asks you for the host'
      : 'Tunnel: the relay does not answer; the agent looks into it');
  }

  function setupPrompt(result, status) {
    const problem = result?.error ? `${result.error}${result.code ? ` [${result.code}]` : ''}` : 'no relay is set up yet';
    const lines = [
      'Set up the tunnel of this kou-conveyor cockpit, so that the kou-conveyor iPhone app and browsers elsewhere reach it through a relay (kou-conveyor-relay) on a host of mine.',
      '',
      `The check of the relay says: ${problem}.`,
    ];
    if (status?.url) lines.push(`tunnel.json names the relay ${status.url}${status.host ? `, installed on ${status.host}` : ''}.`);
    lines.push(
      '',
      'Use the kou-conveyor-tunnel skill. In short: run `"$KOU_CONVEYOR_WEB" tunnel status` to see what is wrong.',
      result?.code === 'not_configured' || !result
        ? '- No relay is installed: ask me which host to install it on — an ssh destination (user@host, or a Host of my ~/.ssh/config) that logs in with a key — and on which port if not 8420, then run `"$KOU_CONVEYOR_WEB" tunnel install <destination>`.'
        : '- A relay is set up but does not answer as it should: find out why (is it running there, is its port open, is it of this version) and fix it, with my say before anything on the host beyond the relay itself.',
      '- Ask me before you change anything on the host beyond installing and running the relay as my user (firewall rules, sudo).',
      '',
      'You are done when `"$KOU_CONVEYOR_WEB" tunnel status` passes: this cockpit then connects to the relay by itself. Tell me then how to pair the iPhone app: the pairing code is in the tunnel dialog (click the planet beside the mark), and `"$KOU_CONVEYOR_WEB" tunnel link` prints the link.',
    );
    return lines.join('\n');
  }

  function command(arg) {
    const word = arg.trim().toLowerCase();
    switch (word) {
      case '': case 'start': case 'on': return start();
      case 'stop': case 'off': return stop();
      case 'status': case 'show': return open();
      case 'setup': case 'install': return setup();
      case 'check': open(); return check();
      default: return toast('/tunnel takes start, stop, status, check or setup', 'error');
    }
  }

  cockpit.provide('tunnel', { status: () => current, open, close, start, stop, setup, refresh });
  cockpit.commands.register({
    name: 'tunnel', args: '[start|stop|status|check|setup]',
    help: 'Carry the cockpit through a relay to the iPhone app and browsers elsewhere: starts it, or has the agent set the relay up',
    order: 155, run: (arg) => command(String(arg ?? '')),
  });
  cockpit.palette.register({ group: 'Actions', icon: '◍', label: 'Tunnel', detail: 'The cockpit on the iPhone and elsewhere, through a relay', order: 185, run: () => open() });
}
