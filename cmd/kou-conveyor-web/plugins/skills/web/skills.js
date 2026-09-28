// skills: the Skills section of the inspector, and /skills — the skills a
// run in the workspace has, from the plugin listing, which follows the
// skill directories as it follows the plugins. The section is a bar that
// opens onto two:
//
//   Project      the workspace's own, which the agent has only there: its
//                .harness/skills and .agents/skills, and its plugins'.
//   System-wide  the user's, which it has in every workspace: skills/ in
//                kou-conveyor's configuration directory, ~/.agents/skills,
//                and the other plugins'.
//
// A project's skill replaces a system-wide one of the same name; a skill
// whose frontmatter says disable-model-invocation loads only when the user
// asks for it. A running agent takes changes up at its next turn. It
// provides the skills service: show().

export default function activate(cockpit) {
  const { h } = cockpit;
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);

  // show opens the inspector at the Skills section, open.
  function show() {
    const inspector = service('inspector');
    if (!inspector) return cockpit.toast('The inspector plugin is off.', 'error');
    inspector.show();
    inspector.expand?.('skills');
    cockpit.renderNow();
    requestAnimationFrame(() => document.querySelector('[data-section="skills"]')?.scrollIntoView({ block: 'start' }));
    return undefined;
  }

  // use puts a request for the skill in the composer, after what is there.
  function use(skill) {
    const composer = service('composer');
    if (!composer?.set) return cockpit.toast('The composer plugin is off.', 'error');
    const before = (composer.value?.() || '').trimEnd();
    composer.set(`${before ? `${before}\n\n` : ''}Use the ${skill.name} skill: `, { focus: true, end: true });
    return undefined;
  }

  // ---------------------------------------------------------------- the section

  // The section keeps its elements, so its bars keep their state and their
  // transitions while the cockpit redraws; what they show is drawn anew only
  // when the listing changes.
  const put = (parent, nodes) => {
    nodes = nodes.filter(Boolean);
    if (parent.childNodes.length !== nodes.length || nodes.some((node, i) => parent.childNodes[i] !== node)) parent.replaceChildren(...nodes);
  };
  const root = h('div', { class: 'skills' });
  const note = h('div', { class: 'skills-note' });
  const foot = h('div', { class: 'skills-foot' });
  const opened = new Set(); // the paths of the skills shown whole
  let bars = null;
  let drawn = null;
  let chrome = '';

  // plainBar stands for a bar when the inspector makes none: a heading and
  // what it holds, always shown.
  function plainBar(title) {
    const heading = h('h3', { class: 'skill-scope', text: title });
    const body = h('div');
    return { node: h('div', null, heading, body), body, isOpen: () => true, title: (text) => { heading.textContent = text; }, hint() {}, meta() {} };
  }

  function barsOf() {
    if (bars) return bars;
    const fold = service('inspector')?.fold;
    const make = (id, title, hint) => (fold ? fold({ id, title, hint, level: 2, beforeOpen: () => draw(true) }) : plainBar(title));
    bars = {
      project: make('skills.project', 'Project', '.harness · .agents'),
      system: make('skills.system', 'System-wide', '~/.agents · kou-conveyor'),
    };
    return bars;
  }

  const within = (skill, directory) => skill.root === directory.path;

  // row is a skill: its name and what it is for, which open onto where it
  // is and what to do with it.
  function row(skill) {
    const whole = opened.has(skill.path);
    const item = h('li', { class: 'skill-row', data: { active: String(skill.active), open: String(whole) } });
    const head = h('button', {
      type: 'button', class: 'skill-head', 'aria-expanded': String(whole),
      onclick: () => {
        const next = item.dataset.open !== 'true';
        if (next) opened.add(skill.path); else opened.delete(skill.path);
        item.dataset.open = String(next);
        head.setAttribute('aria-expanded', String(next));
      },
    },
    h('span', { class: 'skill-line' },
      h('b', { class: 'skill-name', text: skill.name }),
      skill.manual ? h('span', { class: 'skill-tag', data: { tag: 'manual' }, title: 'Its frontmatter keeps the agent from loading it on its own (disable-model-invocation): ask for it by name', text: 'on request' }) : null,
      skill.active ? null : h('span', { class: 'skill-tag', data: { tag: 'replaced' }, title: skill.reason, text: 'replaced' })),
    h('span', { class: 'skill-text', text: skill.description }));
    const explorer = skill.file && cockpit.has('explorer') ? service('explorer') : null;
    item.append(head,
      h('div', { class: 'skill-more' }, h('div', { class: 'skill-more-clip' },
        skill.active ? null : h('p', { class: 'skill-reason', text: skill.reason }),
        h('p', { class: 'skill-path', title: skill.path, text: skill.file || skill.path }),
        h('div', { class: 'skill-actions' },
          h('button', { class: 'act', type: 'button', title: 'Ask the agent for it, in the composer', onclick: () => use(skill) }, 'Use'),
          explorer ? h('button', { class: 'act', type: 'button', title: 'Show its SKILL.md in the Files tab', onclick: () => explorer.open(skill.file) }, 'Open') : null,
          h('button', { class: 'act', type: 'button', onclick: () => cockpit.copy(skill.path, 'Skill path copied') }, 'Copy path')))));
    return item;
  }

  // scopeNodes lists a scope's skills by the directory they come from, the
  // one that wins a name first; a directory that is not there says so.
  function scopeNodes(listing, scope) {
    const directories = listing.directories.filter((directory) => directory.scope === scope);
    if (!directories.length) return [h('p', { class: 'none', text: 'No skill directories.' })];
    return directories.map((directory) => {
      const skills = listing.skills.filter((skill) => within(skill, directory));
      const active = skills.filter((skill) => skill.active).length;
      const errors = directory.errors || [];
      return h('section', { class: 'skill-group', data: { kind: directory.kind, exists: String(directory.exists) } },
        h('div', { class: 'skill-dir', title: directory.path },
          h('span', { class: 'skill-dir-name', text: directory.label }),
          errors.length ? h('span', { class: 'skill-dir-errors', text: `${errors.length} unread` }) : null,
          h('span', { class: 'skill-dir-count', text: !directory.exists ? 'no folder' : skills.length ? String(active) : errors.length ? '0' : 'empty' })),
        skills.length ? h('ol', { class: 'skill-list' }, skills.map(row)) : null,
        errors.length ? h('ul', { class: 'skills-errors' }, errors.map((text) => h('li', { text }))) : null);
    });
  }

  // draw fills the open bars, when the listing changed or force says.
  function draw(force) {
    const listing = cockpit.host.listing()?.skills;
    if (!listing || !bars) return;
    const key = [listing, bars.project.isOpen(), bars.system.isOpen()];
    if (!force && drawn && key.every((value, i) => value === drawn[i])) return;
    drawn = key;
    if (bars.project.isOpen()) put(bars.project.body, scopeNodes(listing, 'project'));
    if (bars.system.isOpen()) put(bars.system.body, scopeNodes(listing, 'system'));
  }

  // errorsOf counts what could not be read: of a scope's directories, or of
  // them all.
  const errorsOf = (listing, scope = '') => listing.directories
    .filter((directory) => !scope || directory.scope === scope)
    .reduce((sum, directory) => sum + (directory.errors?.length || 0), scope ? 0 : listing.errors.length);
  const errorPart = (n) => (n ? { text: `${n} ${n === 1 ? 'error' : 'errors'}`, tone: 'err' } : null);

  const counted = (listing, scope) => {
    const skills = listing.skills.filter((skill) => (skill.scope === 'project') === (scope === 'project'));
    const active = skills.filter((skill) => skill.active).length;
    const replaced = skills.length - active;
    const errors = errorsOf(listing, scope);
    if (!skills.length && !errors) return 'none';
    return [String(active), replaced ? { text: `${replaced} replaced`, title: 'A skill of the same name comes first' } : null, errorPart(errors)];
  };

  // skillUse says whether the agent can load skills: the core plugin brings
  // SkillUse.
  const skillUse = (list) => list.plugins?.find((p) => p.name === 'core')?.active !== false;

  function section() {
    const list = cockpit.host.listing();
    const listing = list?.skills;
    if (!listing) return h('p', { class: 'none', text: list ? 'This server lists no skills: it is older than the page.' : 'Loading skills…' });
    const { project, system } = barsOf();
    project.meta(counted(listing, 'project'));
    system.meta(counted(listing, 'system'));
    const stamp = JSON.stringify([skillUse(list), listing.errors]);
    if (stamp !== chrome) {
      chrome = stamp;
      put(note, [skillUse(list) ? null : h('p', { class: 'skills-off', text: 'The core plugin is off: the agent has no SkillUse, so it has none of these.' })]);
      put(foot, [
        listing.errors.length ? h('ul', { class: 'skills-errors' }, listing.errors.map((text) => h('li', { text }))) : null,
        h('p', { class: 'skills-where', text: "A project's skill replaces a system-wide one of the same name. Changes show at once; a running agent takes them up at its next turn." }),
      ]);
    }
    draw(false);
    put(root, [note, project.node, system.node, foot]);
    return root;
  }

  // meta is what the closed bar says: how many skills the agent has, the
  // project's and the system-wide ones.
  function meta() {
    const list = cockpit.host.listing();
    const listing = list?.skills;
    if (!listing) return '';
    const count = (scope) => listing.skills.filter((skill) => skill.active && (skill.scope === 'project') === (scope === 'project')).length;
    return [
      `${count('project')} project`,
      `${count('system')} system`,
      errorPart(errorsOf(listing)),
      skillUse(list) ? null : { text: 'off', tone: 'warn', title: 'The core plugin is off' },
    ];
  }

  cockpit.inspector.register({ id: 'skills', title: 'Skills', order: 75, fold: true, meta, render: section });

  cockpit.provide('skills', { show });
  cockpit.commands.register({ name: 'skills', help: "The skills the agent has here: the project's and the system-wide ones", order: 241, run: () => show() });
  cockpit.palette.register({ group: 'Actions', icon: '✦', label: 'Skills', detail: "The project's and the system-wide skills", order: 186, run: show });
}
