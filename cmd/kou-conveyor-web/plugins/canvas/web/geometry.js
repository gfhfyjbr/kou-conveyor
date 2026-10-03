// Where things are on the canvas, in world pixels: the nodes' ports, the
// curves of the edges between them, the grid everything snaps to.

export const GRID = 8;
// The height of a node's header; its ports start below it.
export const HEAD = 34;
const PORT_FIRST = 20;
const PORT_STEP = 24;
export const MIN_ZOOM = 0.1;
export const MAX_ZOOM = 2;

export const snap = (v) => Math.round(v / GRID) * GRID;
export const clampZoom = (z) => Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z));

// The glyphs of the kinds of nodes, and the presets that have their own.
const GLYPHS = { terminal: '▣', agent: '◆', source: '⚡', note: '¶', foreman: '◈' };

export function glyphOf(node, kinds) {
  if (node.kind === 'agent') return node.preset === 'foreman' ? GLYPHS.foreman : GLYPHS.agent;
  if (node.kind === 'terminal') return kinds?.harness(node)?.icon || GLYPHS.terminal;
  if (node.kind === 'source') return kinds?.source(node)?.icon || GLYPHS.source;
  return GLYPHS[node.kind] || '·';
}

// The glyphs of the agents the engine knows without a preset (detect.go).
const AGENT_GLYPHS = { claude: '✻', codex: '◎', opencode: '⌬' };

// agentGlyphOf is the glyph of the agent a terminal runs (its status's
// agent): its preset's, else a known agent's.
export function agentGlyphOf(agent, kinds) {
  if (!agent) return '';
  const def = kinds?.harnesses().find((d) => (d.plugin ? `${d.plugin}/${d.id}` : d.id) === agent.id);
  return def?.icon || AGENT_GLYPHS[agent.id] || '◇';
}

// isHarness says a node is a terminal whose preset launches an agent's
// program, as the engine has it (doc.go).
export const isHarness = (node) => node?.kind === 'terminal' && !!node.preset && node.preset !== 'shell' && node.preset !== 'command';

// portsOf names a node's inputs and outputs, as the engine does.
export function portsOf(node, kinds) {
  switch (node.kind) {
    case 'terminal':
      return { inputs: ['in'], outputs: ['out', 'exit'] };
    case 'agent':
      return { inputs: ['in'], outputs: ['out'] };
    case 'source': {
      const outputs = (kinds?.source(node)?.outputs || []).map((p) => p.id);
      return { inputs: [], outputs: outputs.length ? outputs : ['out'] };
    }
    default:
      return { inputs: [], outputs: [] };
  }
}

// portTitle is how a port is named for people.
export function portTitle(node, port, kinds) {
  if (node.kind === 'source') return kinds?.source(node)?.outputs?.find((p) => p.id === port)?.title || port;
  return port;
}

// portPoint is where a port is: inputs on the left edge, outputs on the right.
export function portPoint(node, dir, port, kinds) {
  const { inputs, outputs } = portsOf(node, kinds);
  const list = dir === 'in' ? inputs : outputs;
  const index = Math.max(0, list.indexOf(port));
  return { x: dir === 'in' ? node.x : node.x + node.w, y: node.y + HEAD + PORT_FIRST + index * PORT_STEP };
}

export const portOffset = (index) => HEAD + PORT_FIRST + index * PORT_STEP;

// curve is an edge's path from an output to an input: out to the right,
// in from the left, however the nodes stand.
export function curve(a, b) {
  const d = Math.max(48, Math.abs(b.x - a.x) / 2, Math.min(160, Math.abs(b.y - a.y) / 2));
  return `M${a.x},${a.y} C${a.x + d},${a.y} ${b.x - d},${b.y} ${b.x},${b.y}`;
}

// boundsOf is the box around nodes; null for none.
export function boundsOf(nodes) {
  if (!nodes.length) return null;
  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  for (const n of nodes) {
    left = Math.min(left, n.x);
    top = Math.min(top, n.y);
    right = Math.max(right, n.x + n.w);
    bottom = Math.max(bottom, n.y + n.h);
  }
  return { x: left, y: top, w: right - left, h: bottom - top };
}

export const overlaps = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

// sizeOf is the size a node of a kind and preset starts with, as the
// engine gives it (doc.go).
export function sizeOf(kind, preset = '') {
  switch (kind) {
    case 'terminal':
      return isHarness({ kind, preset }) ? [760, 520] : [760, 460];
    case 'agent':
      return [440, 560];
    case 'source':
      return [320, 220];
    default:
      return [280, 180];
  }
}
