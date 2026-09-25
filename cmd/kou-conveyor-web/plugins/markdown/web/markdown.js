// markdown: the markdown service, which the timeline renders the agent's
// answers and thoughts with:
//
//   render(source, { onCopy }) → a fragment
//   inline(text) → nodes
//   codeBlock(text, language, onCopy) → a figure
//
// Everything reaches the DOM as text nodes, links only for http(s) and
// mailto. A plugin that provides "markdown" too renders them its own way.
import { h } from '/kernel/dom.js';

const SAFE_URL = /^(https?:|mailto:)/i;
const INLINE = new RegExp([
  /(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)/.source, // 1,2 code
  /\*\*(?=\S)([\s\S]*?\S)\*\*/.source, // 3 strong
  /__(?=\S)([\s\S]*?\S)__/.source, // 4 strong
  /~~(?=\S)([\s\S]*?\S)~~/.source, // 5 strike
  /\*(?=[^\s*])([^*]*?[^\s*])\*/.source, // 6 em
  /(?<![\p{L}\p{N}_])_(?=[^\s_])([^_]*?[^\s_])_(?![\p{L}\p{N}_])/.source, // 7 em
  /\[([^\]\n]+)\]\(([^()\s]+)\)/.source, // 8,9 link
  /(https?:\/\/[^\s<>()]*[^\s<>().,;:!?'"\]])/.source, // 10 bare url
].join('|'), 'gu');

export function inline(text) {
  const out = [];
  let last = 0;
  // matchAll iterates a copy of the regex, so the recursion below cannot
  // disturb this loop's position.
  for (const m of text.matchAll(INLINE)) {
    if (m.index > last) out.push(text.slice(last, m.index));
    if (m[2] !== undefined) out.push(h('code', { text: m[2] }));
    else if (m[3] !== undefined || m[4] !== undefined) out.push(h('strong', null, inline(m[3] ?? m[4])));
    else if (m[5] !== undefined) out.push(h('s', null, inline(m[5])));
    else if (m[6] !== undefined || m[7] !== undefined) out.push(h('em', null, inline(m[6] ?? m[7])));
    else if (m[8] !== undefined) {
      out.push(SAFE_URL.test(m[9])
        ? h('a', { href: m[9], target: '_blank', rel: 'noopener noreferrer', title: m[9] }, inline(m[8]))
        : `${m[8]} (${m[9]})`);
    } else if (m[10] !== undefined) {
      out.push(h('a', { href: m[10], target: '_blank', rel: 'noopener noreferrer' }, m[10]));
    }
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

const FENCE = /^\s{0,3}(`{3,}|~{3,})\s*([\w+#.-]*)/;
const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const RULE = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/;
const QUOTE = /^\s{0,3}>\s?(.*)$/;
const ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const TABLE_RULE = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

export function render(source, { onCopy } = {}) {
  const lines = String(source ?? '').replace(/\r\n?/g, '\n').split('\n');
  const root = document.createDocumentFragment();
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) { i++; continue; }

    const fence = FENCE.exec(line);
    if (fence) {
      const body = [];
      const close = new RegExp(`^\\s{0,3}${fence[1][0] === '`' ? '`' : '~'}{${fence[1].length},}\\s*$`);
      for (i++; i < lines.length && !close.test(lines[i]); i++) body.push(lines[i]);
      i++;
      root.append(codeBlock(body.join('\n'), fence[2], onCopy));
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      const level = Math.min(5, heading[1].length + 2);
      root.append(h(`h${level}`, null, inline(heading[2])));
      i++;
      continue;
    }
    if (RULE.test(line)) { root.append(h('hr')); i++; continue; }
    if (QUOTE.test(line)) {
      const body = [];
      for (; i < lines.length && QUOTE.test(lines[i]); i++) body.push(QUOTE.exec(lines[i])[1]);
      root.append(h('blockquote', null, render(body.join('\n'), { onCopy })));
      continue;
    }
    if (ITEM.test(line)) {
      const start = i;
      for (; i < lines.length; i++) {
        if (ITEM.test(lines[i])) continue;
        // Continuation lines are indented; a blank line ends the list unless
        // another item follows.
        if (lines[i].trim() && /^\s{2,}/.test(lines[i])) continue;
        if (!lines[i].trim() && i + 1 < lines.length && ITEM.test(lines[i + 1])) continue;
        break;
      }
      root.append(list(lines.slice(start, i)));
      continue;
    }
    if (line.includes('|') && i + 1 < lines.length && TABLE_RULE.test(lines[i + 1])) {
      const rows = [line];
      for (i += 2; i < lines.length && lines[i].includes('|') && lines[i].trim(); i++) rows.push(lines[i]);
      root.append(table(rows));
      continue;
    }
    const para = [];
    for (; i < lines.length && lines[i].trim(); i++) {
      if (para.length && (FENCE.test(lines[i]) || HEADING.test(lines[i]) || QUOTE.test(lines[i]) || ITEM.test(lines[i]) || RULE.test(lines[i]))) break;
      para.push(lines[i].trim());
    }
    const p = h('p');
    para.forEach((text, n) => { if (n) p.append(h('br')); p.append(...inline(text)); });
    root.append(p);
  }
  return root;
}

function list(lines) {
  // Items nest under the closest previous item with a smaller indent.
  const roots = [];
  const stack = [];
  let last = null;
  for (const line of lines) {
    const m = ITEM.exec(line);
    if (!m) {
      if (last && line.trim()) last.text.push(line.trim());
      continue;
    }
    const item = {
      indent: m[1].replace(/\t/g, '    ').length, ordered: /\d/.test(m[2]),
      start: parseInt(m[2], 10), text: [m[3]], children: [],
    };
    while (stack.length && item.indent <= stack[stack.length - 1].indent) stack.pop();
    (stack.length ? stack[stack.length - 1].children : roots).push(item);
    stack.push(item);
    last = item;
  }
  return listItems(roots);
}

function listItems(items) {
  const frag = document.createDocumentFragment();
  let el = null;
  for (const item of items) {
    if (!el || (el.tagName === 'OL') !== item.ordered) {
      el = h(item.ordered ? 'ol' : 'ul');
      if (item.ordered && item.start !== 1) el.setAttribute('start', item.start);
      frag.append(el);
    }
    const li = h('li');
    let text = item.text.join('\n');
    const task = /^\[([ xX])\]\s+/.exec(text);
    if (task) {
      li.classList.add('task');
      li.append(h('span', { class: 'check', 'aria-hidden': 'true', text: task[1] === ' ' ? '☐' : '☑' }));
      text = text.slice(task[0].length);
    }
    text.split('\n').forEach((part, n) => { if (n) li.append(h('br')); li.append(...inline(part)); });
    if (item.children.length) li.append(listItems(item.children));
    el.append(li);
  }
  return frag;
}

function splitRow(row) {
  let s = row.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|') && !s.endsWith('\\|')) s = s.slice(0, -1);
  return s.split(/(?<!\\)\|/).map((cell) => cell.trim().replace(/\\\|/g, '|'));
}

function table(rows) {
  const [head, ...body] = rows.map(splitRow);
  return h('div', { class: 'table' },
    h('table', null,
      h('thead', null, h('tr', null, head.map((cell) => h('th', null, inline(cell))))),
      h('tbody', null, body.map((row) => h('tr', null, head.map((_, n) => h('td', null, inline(row[n] ?? '')))))),
    ));
}

export function codeBlock(text, language, onCopy) {
  return h('figure', { class: 'code' },
    h('figcaption', null,
      h('span', { text: language || 'text' }),
      onCopy && h('button', { class: 'act', type: 'button', text: 'Copy', onclick: () => onCopy(text) })),
    h('pre', null, h('code', { text })));
}

export default function activate(cockpit) {
  cockpit.provide('markdown', { render, inline, codeBlock });
}
