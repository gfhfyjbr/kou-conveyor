// plugins: the cockpit's plugins, as a section of the inspector — each
// with what it adds, whether it runs and why not, and the version loaded —
// and /plugins. The section is a bar that opens onto two: the project's
// plugins, from the workspace's .harness/plugins, which run in that
// workspace alone and only once it is trusted, and the system-wide ones,
// the user's and the built-in ones, which run in every workspace. Plugins
// change while the page is open: the kernel loads a changed plugin anew by
// itself, and this says so. It provides the plugins service: show(),
// trust(trusted), enable(name, enabled), reload().

// Without these the page cannot show this section to turn them back on.
const ESSENTIAL = new Set(['layout', 'session', 'ui', 'inspector', 'plugins', 'theme']);

export default function activate(cockpit) {
  const { h, fmt } = cockpit;
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const toast = (text, kind = 'info', key = '') => cockpit.toast(text, kind, key);

  async function trust(trusted) {
    try {
      await cockpit.api('/plugins/trust', { method: 'PUT', body: { trusted } });
    } catch (error) {
      return toast(error.message, 'error');
    }
    toast(trusted ? "The workspace's plugins run now, in this cockpit and in the runs it starts" : "The workspace's plugins are off");
    return reload({ quiet: true });
  }

  async function enable(name, enabled) {
    try {
      await cockpit.api(`/plugins/${encodeURIComponent(name)}/enabled`, { method: 'PUT', body: { enabled } });
    } catch (error) {
      return toast(error.message, 'error');
    }
    toast(enabled ? `Plugin ${name} is on` : `Plugin ${name} is off in every workspace`);
    return reload({ quiet: true });
  }

  // reload asks for the plugins again; alone, it loads each one anew.
  async function reload({ quiet = false } = {}) {
    try {
      const listing = await cockpit.host.reload({ force: !quiet });
      if (!quiet) toast(`${listing.plugins.filter((p) => p.active).length} plugins loaded`);
    } catch (error) {
      toast(`Plugins did not load: ${error.message}`, 'error');
    }
  }

  // show opens the inspector at the Plugins section, open.
  function show() {
    const inspector = service('inspector');
    if (!inspector) return toast('The inspector plugin is off.', 'error');
    inspector.show();
    inspector.expand?.('plugins');
    cockpit.renderNow();
    requestAnimationFrame(() => document.querySelector('[data-section="plugins"]')?.scrollIntoView({ block: 'start' }));
    return undefined;
  }

  // ---------------------------------------------------------------- the section

  // The section keeps its elements, so its bars keep their state and their
  // transitions while the cockpit redraws; what the bars show is drawn anew
  // only when it changes.
  const put = (parent, nodes) => {
    nodes = nodes.filter(Boolean);
    if (parent.childNodes.length !== nodes.length || nodes.some((node, i) => parent.childNodes[i] !== node)) parent.replaceChildren(...nodes);
  };
  const root = h('div', { class: 'plugins' });
  const notices = h('div', { class: 'plugin-notices' });
  const foot = h('div', { class: 'plugin-foot' });
  let bars = null;
  let drawn = '';
  let chrome = '';

  // plainBar stands for a bar when the inspector makes none: a heading and
  // what it holds, always shown.
  function plainBar(title) {
    const heading = h('h3', { class: 'plugin-group', text: title });
    const body = h('div');
    return { node: h('div', null, heading, body), body, isOpen: () => true, title: (text) => { heading.textContent = text; }, hint() {}, meta() {} };
  }

  function barsOf() {
    if (bars) return bars;
    const fold = service('inspector')?.fold;
    const make = (id, title, hint) => (fold ? fold({ id, title, hint, level: 2, beforeOpen: () => draw(true) }) : plainBar(title));
    bars = {
      project: make('plugins.project', 'Project', '.harness/plugins'),
      system: make('plugins.system', 'System-wide', 'yours · built in'),
    };
    return bars;
  }

  const projectOf = (list) => list.plugins.filter((p) => p.source === 'workspace');
  // The user's plugins come before the built-in ones they may replace.
  const systemOf = (list) => [...list.plugins.filter((p) => p.source === 'user'), ...list.plugins.filter((p) => p.source === 'builtin')];

  function row(p, loaded, failures) {
    const running = loaded.get(p.name);
    const adds = [
      p.tools.length && `${p.tools.length} ${p.tools.length === 1 ? 'tool' : 'tools'}`,
      p.commands.length && p.commands.map((c) => `/${c.name}`).join(' '),
      p.skills && 'skills', p.instructions && 'instructions', (p.script || p.style) && 'interface',
    ].filter(Boolean).join(' · ');
    const essential = p.source === 'builtin' && ESSENTIAL.has(p.name);
    const toggle = essential || p.reason === 'the workspace is not trusted' || p.name === 'core' ? null
      : h('button', {
        class: 'act', type: 'button', title: p.active ? 'Turn the plugin off in every workspace' : 'Turn the plugin on',
        onclick: () => enable(p.name, !p.active),
      }, p.active || p.reason !== 'turned off' ? 'Turn off' : 'Turn on');
    const problem = running?.error || running?.stale?.error || (failures[p.name] && Date.now() - failures[p.name].at < 60_000 ? failures[p.name].message : '');
    return h('li', { class: 'plugin-row', data: { active: String(p.active), live: p.live ? 'true' : null } },
      h('div', { class: 'plugin-head' },
        h('b', { text: p.name }), p.version ? h('span', { class: 'plugin-version', text: p.version }) : null,
        p.live && p.active ? h('span', { class: 'plugin-live', title: 'Read from disk: changes show at once', text: 'live' }) : null,
        h('span', { class: `plugin-source ${p.source}`, text: p.source }), toggle),
      p.description ? h('p', { class: 'plugin-text', text: p.description }) : null,
      adds ? h('p', { class: 'plugin-adds', text: adds }) : null,
      p.active ? null : h('p', { class: 'plugin-reason', text: p.reason }),
      running && !running.broken && (p.script || p.style)
        ? h('p', { class: 'plugin-loaded', text: `loaded ${fmt.ago(running.since) === 'now' ? 'just now' : `${fmt.ago(running.since)} ago`} · ${running.code_version?.slice(0, 7) || ''}` })
        : null,
      problem ? h('p', { class: 'plugin-problem', text: running?.stale ? `the new version did not load: ${problem}` : problem }) : null);
  }

  // The project's: a workspace's plugins run code from wherever the
  // workspace came from, so they wait for it to be trusted.
  function projectNodes(list, loaded, failures) {
    const project = projectOf(list);
    const untrusted = !list.trusted && project.length > 0;
    return [
      untrusted ? h('div', { class: 'plugin-trust' },
        h('p', {
          text: project.length === 1
            ? 'This workspace has a plugin. It runs code from the workspace, so it stays off until you trust the workspace.'
            : `This workspace has ${project.length} plugins. They run code from the workspace, so they stay off until you trust it.`,
        }),
        h('button', { class: 'act strong', type: 'button', onclick: () => trust(true) }, 'Trust this workspace')) : null,
      project.length ? h('ol', { class: 'plugin-list' }, project.map((p) => row(p, loaded, failures)))
        : h('p', { class: 'none plugin-empty', text: 'This workspace has no plugins of its own.' }),
      list.trusted && project.length ? h('div', { class: 'ins-actions' },
        h('button', { class: 'act', type: 'button', onclick: () => trust(false) }, 'Stop trusting')) : null,
      h('p', { class: 'plugin-where', text: `From ${list.workspace_directory}, and nowhere else in the workspace: they run in this workspace alone, once it is trusted.` }),
    ];
  }

  function systemNodes(list, loaded, failures) {
    return [
      h('ol', { class: 'plugin-list' }, systemOf(list).map((p) => row(p, loaded, failures))),
      h('p', {
        class: 'plugin-where',
        text: `Yours: ${list.user_directory}${list.builtin_directory ? `; built in: ${list.builtin_directory}` : ''}. They run in every workspace.`,
      }),
    ];
  }

  // listings tells the listings apart, as they come.
  const listings = new WeakMap();
  let listingCount = 0;
  const listingNumber = (list) => {
    if (!listings.has(list)) listings.set(list, ++listingCount);
    return listings.get(list);
  };

  // draw fills the open bars, when what they show changed or force says.
  function draw(force) {
    const list = cockpit.host.listing();
    if (!list || !bars) return;
    const loaded = cockpit.host.loaded();
    const failures = cockpit.host.failures();
    const recent = Object.entries(failures).filter(([, f]) => Date.now() - f.at < 60_000).map(([name, f]) => [name, f.at]);
    const stamp = JSON.stringify([listingNumber(list), loaded.map((i) => [i.name, i.code_version, i.error, !!i.broken, i.stale?.error || '', i.since]),
      recent, Math.floor(Date.now() / 30_000), bars.project.isOpen(), bars.system.isOpen()]);
    if (!force && stamp === drawn) return;
    drawn = stamp;
    const byName = new Map(loaded.map((item) => [item.name, item]));
    if (bars.project.isOpen()) put(bars.project.body, projectNodes(list, byName, failures));
    if (bars.system.isOpen()) put(bars.system.body, systemNodes(list, byName, failures));
  }

  function section() {
    const list = cockpit.host.listing();
    if (!list) return h('p', { class: 'none', text: 'Loading plugins…' });
    const { project, system } = barsOf();
    const own = projectOf(list);
    const others = systemOf(list);
    const untrusted = !list.trusted && own.length > 0;
    project.meta(own.length ? [String(own.length), untrusted ? { text: 'not trusted', tone: 'warn' } : `${own.filter((p) => p.active).length} on`] : 'none');
    system.meta(`${others.filter((p) => p.active).length}/${others.length} on`);
    // What is around the bars is drawn anew when it changes, too, so a
    // button keeps the focus.
    const build = cockpit.hot.data.build;
    const server = list.server;
    const stamp = JSON.stringify([server, build, cockpit.host.safe, list.errors]);
    if (stamp !== chrome) {
      chrome = stamp;
      put(notices, [
        server?.started_at ? h('p', {
          class: 'plugin-server', title: 'The server builds itself anew as its Go code changes',
          text: `Server · started ${fmt.clock(server.started_at)} · build ${server.build}${server.rebuild ? ' · follows its Go code' : ''}`,
        }) : null,
        build && build.state !== 'idle' ? h('div', { class: 'plugin-build', data: { state: build.state } },
          h('p', { text: `Server: ${build.message}` }),
          build.output ? h('pre', { text: build.output }) : null) : null,
        cockpit.host.safe ? h('p', { class: 'plugin-safe', text: 'Safe mode: only the built-in plugins run. Open the page without ?safe to run the others.' }) : null,
      ]);
      put(foot, [
        list.errors.length ? h('ul', { class: 'plugin-errors' }, list.errors.map((text) => h('li', { text }))) : null,
        h('div', { class: 'ins-actions' },
          h('button', { class: 'act', type: 'button', title: 'Load every plugin anew', onclick: () => reload() }, 'Reload')),
        h('p', { class: 'plugin-where', text: 'Changes to plugins show at once.' }),
      ]);
    }
    draw(false);
    put(root, [notices, project.node, system.node, foot]);
    return root;
  }

  // meta is what the closed bar says: how many run, the project's, and
  // what needs a look.
  function meta() {
    const list = cockpit.host.listing();
    if (!list) return '';
    const own = projectOf(list);
    const build = cockpit.hot.data.build;
    return [
      `${list.plugins.filter((p) => p.active).length} on`,
      own.length ? (!list.trusted ? { text: `${own.length} not trusted`, tone: 'warn' } : `${own.length} project`) : null,
      list.errors.length ? { text: `${list.errors.length} ${list.errors.length === 1 ? 'error' : 'errors'}`, tone: 'err' } : null,
      build?.state === 'failed' ? { text: 'build failed', tone: 'err' } : null,
    ];
  }

  cockpit.inspector.register({ id: 'plugins', title: 'Plugins', order: 80, fold: true, meta, render: section });
  cockpit.on('plugins', () => cockpit.render());

  // ---------------------------------------------------------------- word of changes

  // Plugins loaded anew in one go are named in one message.
  let reloaded = [];
  let timer = 0;
  function announce(name, what) {
    reloaded.push(name);
    clearTimeout(timer);
    timer = setTimeout(() => {
      const names = [...new Set(reloaded)];
      reloaded = [];
      toast(names.length === 1 ? `${what === 'style' ? 'Styles of' : 'Plugin'} ${names[0]} ${what === 'style' ? 'updated' : 'reloaded'}` : `Plugins reloaded: ${names.join(', ')}`, 'info', 'plugin-reload');
    }, 150);
  }
  cockpit.onDispose(() => clearTimeout(timer));
  cockpit.on('plugin-reloaded', (name) => { if (name !== cockpit.plugin.name) announce(name, 'code'); });
  cockpit.on('plugin-restyled', (name) => announce(name, 'style'));
  cockpit.on('kernel-changed', () => toast('The page itself changed: loading it again'));

  // The server's own builds, as its Go code changes.
  const hot = cockpit.hot.data;
  hot.build ??= null;
  const BUILD_WORDS = {
    building: ['The Go code changed: building the server anew…', 'info'],
    waiting: ['Built. The server takes the new build up once the agents at work finish.', 'info'],
    restarting: ['Restarting the server with the new build…', 'info'],
  };
  cockpit.on('server', (status) => {
    hot.build = status;
    if (status.state === 'failed') {
      // The compiler's first error, rather than the package it names first.
      const lines = (status.output || '').split('\n').map((line) => line.trim()).filter((line) => line && line !== '…');
      const first = lines.find((line) => /\.go:\d+/.test(line)) || lines.find((line) => !line.startsWith('#')) || status.message;
      toast(`The build failed: ${first}`, 'error', 'server');
    }
    else if (BUILD_WORDS[status.state]) toast(BUILD_WORDS[status.state][0], BUILD_WORDS[status.state][1], 'server');
    cockpit.render();
  });
  cockpit.on('server-restarted', () => {
    hot.build = null;
    toast('The server runs the new build', 'info', 'server');
    cockpit.render();
  });
  if (cockpit.hot.reloaded) announce(cockpit.plugin.name, 'code');

  cockpit.provide('plugins', { show, trust, enable, reload });
  cockpit.commands.register({
    name: 'plugins', args: '[trust|untrust|reload]', help: "The workspace's plugins: list them, trust the workspace's, or load them all anew", order: 240,
    complete: () => {
      const list = cockpit.host.listing();
      return [
        { value: 'trust', label: 'trust', detail: list?.trusted ? 'the workspace is trusted' : `run the workspace's ${list?.workspace_plugins || 0} plugins` },
        { value: 'untrust', label: 'untrust', detail: "stop running the workspace's plugins" },
        { value: 'reload', label: 'reload', detail: 'load every plugin anew (changed ones load by themselves)' },
      ];
    },
    run: (arg) => {
      switch (String(arg).trim()) {
        case '': return show();
        case 'trust': return trust(true);
        case 'untrust': return trust(false);
        case 'reload': return reload();
        default: return toast('/plugins takes trust, untrust or reload', 'error');
      }
    },
  });
  cockpit.palette.register({ group: 'Actions', icon: '◇', label: 'Plugins', detail: 'What each plugin adds; turn them on and off', order: 185, run: show });
}
