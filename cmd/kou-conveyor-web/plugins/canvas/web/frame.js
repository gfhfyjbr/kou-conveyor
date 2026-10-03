// A node's frame: what every node has around its body — the header with
// its glyph, title, status and menu, its ports on the left and right edges,
// the footer that says where it works (worktree, folder, program), the
// badge of the messages that wait for it, the bar of a node an agent
// proposed, and the corner it is resized by. The surface places frames
// and handles what is done to them; the body is the kind's (nodes/).
//
// A shell that runs an agent the user started there (Claude Code, Codex…)
// says so in its header: the agent's glyph, and a tag with its name.
import { glyphOf, agentGlyphOf, portsOf, portTitle, portOffset } from './geometry.js';
import { shownState, stateWord, atWork } from './status.js';

export function createFrame(node, env) {
  const { h, fmt } = env;
  const glyph = h('span', { class: 'cv-glyph', 'aria-hidden': 'true' });
  const title = h('span', { class: 'cv-title' });
  const kind = h('span', { class: 'cv-kind' });
  const runs = h('span', { class: 'cv-runs', hidden: true });
  const word = h('b');
  const time = h('time');
  const state = h('span', { class: 'cv-state' }, h('i', { 'aria-hidden': 'true' }), word, time);
  const typing = h('span', { class: 'cv-typing', hidden: true });
  const more = h('button', { class: 'cv-more', type: 'button', title: 'Node actions', 'aria-label': 'Node actions', 'aria-haspopup': 'menu', 'aria-expanded': 'false' }, '⋯');
  const head = h('header', { class: 'cv-head' }, glyph, title, kind, runs, typing, state, more);
  const inputs = h('div', { class: 'cv-ports', data: { dir: 'in' } });
  const outputs = h('div', { class: 'cv-ports', data: { dir: 'out' } });
  const body = h('div', { class: 'cv-body' });
  const foot = h('footer', { class: 'cv-foot' });
  const pending = h('button', { class: 'cv-pending', type: 'button', hidden: true });
  const approve = h('button', { class: 'act strong', type: 'button', data: { act: 'approve' } }, 'Approve');
  const discard = h('button', { class: 'act', type: 'button', data: { act: 'discard' } }, 'Discard');
  const proposalText = h('span');
  const proposal = h('div', { class: 'cv-proposal', hidden: true }, proposalText, approve, discard);
  const resize = h('div', { class: 'cv-resize', title: 'Drag to resize', 'aria-hidden': 'true' });
  const el = h('article', { class: 'cv-node', data: { id: node.id, kind: node.kind }, 'aria-label': node.title || node.id },
    head, inputs, outputs, body, foot, proposal, pending, resize);

  let ports = '';
  let since = 0;
  let stateName = '';
  let typingTimer = 0;

  function drawPorts(n) {
    const { inputs: ins, outputs: outs } = portsOf(n, env.kinds);
    const key = `${ins.join(',')}|${outs.join(',')}`;
    if (key === ports) return;
    ports = key;
    // The page's policy refuses style attributes: the place is set as a
    // property.
    const port = (dir, name, index) => {
      const el = h('span', {
        class: 'cv-port', data: { dir, port: name, node: n.id },
        title: `${dir === 'in' ? 'Input' : 'Output'} ${portTitle(n, name, env.kinds)} — drag to wire it`,
      }, h('i', { 'aria-hidden': 'true' }), h('b', { text: portTitle(n, name, env.kinds) }));
      el.style.top = `${portOffset(index)}px`;
      return el;
    };
    inputs.replaceChildren(...ins.map((name, i) => port('in', name, i)));
    outputs.replaceChildren(...outs.map((name, i) => port('out', name, i)));
  }

  // footOf is where a node works: its worktree's branch, its folder, and
  // what runs there.
  function footOf(n, status, terminal) {
    const parts = [];
    const runtime = n.runtime || {};
    if (runtime.branch) parts.push(`⎇ ${runtime.branch}`);
    if (n.kind === 'terminal') {
      const cwd = env.short(terminal?.cwd || runtime.worktree || n.config?.cwd || '');
      if (cwd) parts.push(cwd);
      const def = env.kinds.harness(n);
      const program = terminal?.running || (def?.program && def.launch !== 'shell' ? def.program : '');
      if (program) parts.push(`▸ ${program}`);
    } else if (n.kind === 'source') {
      const def = env.kinds.source(n);
      parts.push(def ? `${def.title}${n.plugin ? ` · ${n.plugin}` : ''}` : `${n.plugin ? `${n.plugin}/` : ''}${n.preset}`);
    }
    if (status?.activity && (status.state === 'busy' || status.state === 'starting')) parts.push(status.activity);
    return parts.join(' · ');
  }

  // place puts the frame where the node is, as large as it is: all a drag
  // changes.
  function place(n) {
    el.style.transform = `translate(${n.x}px, ${n.y}px)`;
    el.style.width = `${n.w}px`;
    el.style.height = `${n.h}px`;
  }

  function update(n, { status, terminal, pendingCount, awaiting, selected, kindLabel }) {
    el.dataset.kind = n.kind;
    el.dataset.preset = n.preset || '';
    el.dataset.selected = selected ? 'true' : '';
    el.dataset.proposed = n.proposed ? 'true' : '';
    el.setAttribute('aria-label', n.title || n.id);
    place(n);
    el.style.zIndex = String((n.z || 0) + (selected ? 100000 : 0));
    // An agent found in the shell: its glyph, and its name in a tag.
    const agent = n.kind === 'terminal' && status?.agent?.detected ? status.agent : null;
    const g = agent ? agentGlyphOf(agent, env.kinds) : glyphOf(n, env.kinds);
    if (glyph.textContent !== g) glyph.textContent = g;
    const t = n.title || 'Untitled';
    if (title.textContent !== t) title.textContent = t;
    title.title = `${t} — double-click to rename`;
    if (kind.textContent !== kindLabel) kind.textContent = kindLabel;
    runs.hidden = !agent;
    el.dataset.agent = agent ? 'true' : '';
    if (agent) {
      const name = agent.title || agent.id;
      if (runs.textContent !== name) runs.textContent = name;
      runs.title = `${name} runs in this shell${agent.pid ? ` (pid ${agent.pid})` : ''}: what is sent to the node goes to it as a prompt, and what it shows once it falls quiet is the answer`;
    }
    drawPorts(n);
    setStatus(status);
    const f = footOf(n, status, terminal);
    if (foot.textContent !== f) foot.textContent = f;
    foot.title = f;
    foot.hidden = !f;
    // What waits for it: messages, those to approve first.
    pending.hidden = !pendingCount;
    if (pendingCount) {
      const first = awaiting[0];
      const text = awaiting.length
        ? `${awaiting.length} to approve${first?.from?.node ? ` · from ${env.titleOf(first.from.node)}` : ''}`
        : `${pendingCount} pending`;
      if (pending.textContent !== text) pending.textContent = text;
      pending.dataset.approve = awaiting.length ? 'true' : '';
      pending.title = awaiting.length ? 'Messages wait for your approval: open them' : 'Messages wait until it is ready for them';
    }
    proposal.hidden = !n.proposed;
    if (n.proposed) proposalText.textContent = `Proposed by «${env.titleOf(n.created_by)}»`;
  }

  // setStatus shows a status: a command that runs on quietly shows as
  // running, without the blinking and the line of work (status.js).
  function setStatus(status) {
    const st = status || { state: 'stopped' };
    const shown = shownState(st);
    if (el.dataset.status !== shown) el.dataset.status = shown;
    const w = stateWord(shown);
    if (word.textContent !== w) word.textContent = w;
    stateName = shown;
    since = st.since ? Date.parse(st.since) : 0;
    if (shown === 'running') state.title = `running: it has shown nothing for a while${st.detail ? ` · ${st.detail}` : ''}`;
    else state.title = st.detail ? `${w}: ${st.detail}` : w;
    tick();
  }

  // tick has the time of a status at work say how long it has lasted.
  function tick() {
    const text = since && atWork(stateName) ? fmt.duration(Date.now() - since) : '';
    if (time.textContent !== text) time.textContent = text;
  }

  function showTyping(who) {
    clearTimeout(typingTimer);
    typing.textContent = `⌨ ${who} typing…`;
    typing.hidden = false;
    typingTimer = setTimeout(() => { typing.hidden = true; }, 1500);
  }

  return {
    id: node.id,
    el, head, title, body, more, pending, resize,
    update,
    place,
    setStatus,
    tick,
    showTyping,
    busy: () => atWork(stateName),
    dispose() {
      clearTimeout(typingTimer);
      el.remove();
    },
  };
}
