// The web part of git-glance. The cockpit calls the default export with its
// plugin API (see docs/plugins.md); whatever it registers goes away when the
// workspace does, or when the plugin changes and is loaded anew.
export default function activate(cockpit) {
  const { h } = cockpit;

  // The GitGlance result is JSON; a call shows it as a card.
  const parse = (entry) => {
    try {
      return JSON.parse(entry.tool?.output || '');
    } catch {
      return null;
    }
  };
  cockpit.tools.register('GitGlance', {
    summary(entry) {
      const state = parse(entry);
      if (!state) return entry.tool?.state === 'running' ? 'looking…' : '';
      if (state.error) return state.error;
      return `${state.branch} · ${plural(state.changes.length, 'change')} · ${plural(state.commits.length, 'commit')}`;
    },
    render(entry) {
      const state = parse(entry);
      return state && !state.error ? card(h, state) : null; // null keeps the cockpit's own view
    },
  });

  // The inspector shows the latest glance of the session.
  const latest = () => cockpit.entries().filter((e) => e.kind === 'tool' && e.tool?.name === 'GitGlance').map(parse).filter(Boolean).pop();
  cockpit.inspector.register({
    id: 'git',
    title: 'Git',
    render(view) {
      const state = latest();
      const glance = () => cockpit.prompt('Call GitGlance and tell me the state of the repository in two sentences.');
      return h('div', { class: 'git-glance' },
        state && !state.error ? card(h, state) : h('p', { class: 'none', text: view?.fresh ? 'No glance yet' : 'No glance in this session yet' }),
        h('div', { class: 'ins-actions' }, cockpit.ui.button('Glance now', glance, 'Ask the agent to call GitGlance')));
    },
  });
  cockpit.on('finish', () => cockpit.inspector.refresh());

  // The branch of the latest glance, in the bar over the transcript: a part
  // of the page of its own, beside the run's state (the slot bar.end).
  const chip = h('span', { class: 'badge git-chip', hidden: true, title: 'The branch of the latest glance' });
  cockpit.ui.mount('bar.end', { id: 'git-branch', order: 25, node: chip });
  const showBranch = () => {
    const state = latest();
    chip.hidden = !state || !!state.error;
    if (!chip.hidden) chip.textContent = `⎇ ${state.branch}`;
  };
  cockpit.on('entry', showBranch);
  cockpit.on('session', showBranch);
  showBranch();

  cockpit.palette.register({
    icon: '±', label: 'Review the uncommitted changes', detail: 'git-glance',
    run: () => cockpit.prompt('Review the uncommitted changes of this repository: call GitGlance first.'),
  });
}

function plural(n, word) {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

function card(h, state) {
  return h('div', { class: 'git-card' },
    h('div', { class: 'git-branch' }, h('span', { class: 'label', text: 'Branch' }), h('code', { text: state.branch })),
    state.changes.length
      ? h('ul', { class: 'git-changes' }, state.changes.map((c) => h('li', null, h('b', { class: `git-status s-${c.status[0] || 'x'}`, text: c.status }), h('code', { text: c.path }))))
      : h('p', { class: 'git-clean', text: 'Nothing to commit' }),
    state.commits.length
      ? h('ol', { class: 'git-commits' }, state.commits.map((c) => h('li', null, h('code', { text: c.hash }), h('span', { text: c.subject }))))
      : null);
}
