// code: a file's text, highlighted, with its lines numbered. The server
// sends the text once and its highlighting beside it as runs — a length in
// UTF-16 units and a class, pair by pair — so nothing of the file is ever
// HTML: each run becomes a text node in a span of its class. A long file
// draws only the lines in view, as it scrolls; a short one draws them all,
// so selecting and finding in it work as on any page.

// Up to this many lines a file is drawn whole.
const WHOLE = 4000;
// The lines drawn beyond those in view, above and below.
const OVERSCAN = 40;

// createCode makes a view of text: runs and classes its highlighting,
// options { wrap, onLine(line, event) } how it draws and says where a line
// number was clicked.
export function createCode(h, text, runs, classes, options = {}) {
  // Where each line starts.
  const starts = [0];
  for (let at = text.indexOf('\n'); at >= 0; at = text.indexOf('\n', at + 1)) starts.push(at + 1);
  if (starts.length > 1 && starts[starts.length - 1] === text.length) starts.pop(); // the last newline ends the text
  const count = starts.length;
  const lineEnd = (n) => (n + 1 < count ? starts[n + 1] - 1 : text.endsWith('\n') ? text.length - 1 : text.length);

  // Where each run starts.
  let runStarts = new Float64Array(0);
  let runCount = 0;
  function setRuns(list) {
    runs = list || [];
    runCount = Math.floor(runs.length / 2);
    runStarts = new Float64Array(runCount + 1);
    for (let i = 0, at = 0; i < runCount; i++) {
      runStarts[i] = at;
      at += runs[i * 2];
      runStarts[i + 1] = at;
    }
  }
  setRuns(runs);

  // runAt finds the run a position is in: the last that starts at or
  // before it.
  function runAt(pos) {
    let lo = 0;
    let hi = runCount - 1;
    if (hi < 0 || pos >= runStarts[runCount]) return runCount;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (runStarts[mid] <= pos) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  // The widest line, for the width to scroll in (tabs count four).
  let widest = 0;
  for (let n = 0; n < count; n++) {
    const length = lineEnd(n) - starts[n];
    if (length > widest) widest = length;
  }
  const digits = String(count).length;

  const selected = { from: 0, to: 0 };
  const rows = h('div', { class: 'code-rows' });
  const sizer = h('div', { class: 'code-sizer' }, rows);
  const root = h('div', { class: 'code', tabindex: '0' }, sizer);
  root.style.setProperty('--digits', String(Math.max(3, digits)));
  root.style.setProperty('--cols', String(widest + 8));
  const whole = count <= WHOLE;
  let wrap = !!options.wrap && whole;
  root.classList.toggle('wrap', wrap);

  function drawLine(n) {
    const start = starts[n];
    const end = lineEnd(n);
    const content = h('span', { class: 'code-text' });
    let pos = start;
    let r = runAt(pos);
    while (pos < end) {
      if (r >= runCount) {
        content.append(text.slice(pos, end));
        break;
      }
      const runEnd = runStarts[r + 1];
      const stop = Math.min(end, runEnd);
      if (stop > pos) {
        const cls = classes[runs[r * 2 + 1]];
        const piece = text.slice(pos, stop);
        content.append(cls ? h('span', { class: `tk-${cls}`, text: piece }) : piece);
      }
      pos = stop;
      if (pos >= runEnd) r++;
    }
    if (start === end) content.append('\u200b'); // an empty line keeps its height when copied
    const number = h('span', { class: 'code-no', 'data-line': String(n + 1) });
    const row = h('div', { class: 'code-line', data: { n: String(n + 1) } }, number, content);
    if (selected.from && n + 1 >= selected.from && n + 1 <= selected.to) row.classList.add('sel');
    return row;
  }

  let first = -1;
  let last = -1;
  let lineHeight = 0;
  function measure() {
    if (lineHeight) return lineHeight;
    const probe = drawLine(0);
    rows.append(probe);
    lineHeight = probe.getBoundingClientRect().height || 19;
    probe.remove();
    return lineHeight;
  }

  function draw(force = false) {
    if (whole) {
      if (force || !rows.childElementCount) {
        const nodes = new Array(count);
        for (let n = 0; n < count; n++) nodes[n] = drawLine(n);
        rows.replaceChildren(...nodes);
        rows.style.removeProperty('--top');
        sizer.style.removeProperty('--height');
      }
      return;
    }
    const lh = measure();
    sizer.style.setProperty('--height', `${count * lh}px`);
    const top = root.scrollTop;
    const view = root.clientHeight || 600;
    const from = Math.max(0, Math.floor(top / lh) - OVERSCAN);
    const to = Math.min(count, Math.ceil((top + view) / lh) + OVERSCAN);
    if (!force && from === first && to === last) return;
    first = from;
    last = to;
    const nodes = [];
    for (let n = from; n < to; n++) nodes.push(drawLine(n));
    rows.replaceChildren(...nodes);
    rows.style.setProperty('--top', `${from * lh}px`);
  }

  let frame = 0;
  root.addEventListener('scroll', () => {
    if (whole || frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      draw();
    });
  }, { passive: true });

  root.addEventListener('click', (event) => {
    const number = event.target instanceof Element ? event.target.closest('.code-no') : null;
    if (!number) return;
    const line = Number(number.dataset.line);
    if (event.shiftKey && selected.from) {
      const anchor = selected.anchor || selected.from;
      select(Math.min(anchor, line), Math.max(anchor, line), anchor);
    } else if (selected.from === line && selected.to === line) {
      select(0, 0);
    } else {
      select(line, line, line);
    }
    options.onLine?.(selected.from ? { from: selected.from, to: selected.to } : null);
  });

  function select(from, to, anchor = from) {
    selected.from = from;
    selected.to = to;
    selected.anchor = anchor;
    for (const row of rows.children) {
      const n = Number(row.dataset.n);
      row.classList.toggle('sel', !!from && n >= from && n <= to);
    }
  }

  return {
    node: root,
    lines: count,
    whole,
    draw,
    // highlight takes the rest of the highlighting, which came later.
    highlight(list) {
      setRuns(list);
      draw(true);
    },
    setWrap(on) {
      wrap = !!on && whole;
      root.classList.toggle('wrap', wrap);
    },
    // reveal scrolls a line into view.
    reveal(line) {
      const n = Math.max(1, Math.min(count, line)) - 1;
      if (whole) rows.children[n]?.scrollIntoView({ block: 'center' });
      else root.scrollTop = Math.max(0, n * measure() - root.clientHeight / 3);
    },
    select,
    selection: () => (selected.from ? { from: selected.from, to: selected.to } : null),
    // text of the lines selected, or all of it.
    text: () => {
      if (!selected.from) return text;
      return text.slice(starts[selected.from - 1], lineEnd(selected.to - 1));
    },
  };
}
