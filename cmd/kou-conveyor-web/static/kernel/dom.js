// The kernel's DOM helpers, which every plugin gets as cockpit.h, cockpit.fmt
// and so on; a plugin's own modules may import them from /kernel/dom.js too.
// Everything that comes from the runner is untrusted, so it only ever
// reaches the DOM as text nodes or through validated attributes — never as
// HTML.

// h builds an element: h('button', { class, text, data: {...}, onclick, ...attributes }, ...children).
export function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value == null || value === false) continue;
    if (key === 'class') el.className = value;
    else if (key === 'text') el.textContent = value;
    else if (key === 'data') {
      for (const [name, v] of Object.entries(value)) if (v != null) el.dataset[name] = v;
    }
    else if (key.startsWith('on')) el.addEventListener(key.slice(2), value);
    else el.setAttribute(key, value === true ? '' : String(value));
  }
  for (const child of children.flat(Infinity)) {
    if (child == null || child === false) continue;
    el.append(child instanceof Node ? child : String(child));
  }
  return el;
}

// svg builds an SVG element from trusted markup of the plugin's own, such as
// an icon: h cannot, as SVG lives in its own namespace.
export function svg(markup) {
  const template = document.createElement('template');
  template.innerHTML = String(markup).trim();
  return template.content.firstElementChild;
}

const pad = (n) => String(n).padStart(2, '0');

export const fmt = Object.freeze({
  clock(value) {
    const d = new Date(value);
    return Number.isNaN(d.getTime()) ? '' : `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  },
  short(value) {
    const d = new Date(value);
    return Number.isNaN(d.getTime()) ? '' : `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  },
  stamp(value) {
    const d = new Date(value);
    return Number.isNaN(d.getTime()) ? '' : d.toLocaleString();
  },
  ago(value) {
    const d = new Date(value);
    const seconds = (Date.now() - d.getTime()) / 1000;
    if (!Number.isFinite(seconds)) return '';
    if (seconds < 45) return 'now';
    if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
    if (seconds < 86400) return `${Math.round(seconds / 3600)}h`;
    if (seconds < 86400 * 7) return `${Math.round(seconds / 86400)}d`;
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  },
  duration(ms) {
    if (!Number.isFinite(ms) || ms < 0) return '';
    if (ms < 1000) return `${(ms / 1000).toFixed(1)}s`;
    const s = Math.floor(ms / 1000);
    if (s < 60) return `${s}s`;
    if (s < 3600) return `${Math.floor(s / 60)}m ${pad(s % 60)}s`;
    return `${Math.floor(s / 3600)}h ${pad(Math.floor(s / 60) % 60)}m`;
  },
  timer(ms) {
    const s = Math.max(0, Math.floor(ms / 1000));
    return s >= 3600 ? `${Math.floor(s / 3600)}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}` : `${pad(Math.floor(s / 60))}:${pad(s % 60)}`;
  },
  bytes(n) {
    if (!Number.isFinite(n)) return '';
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB`;
    return `${(n / 1024 / 1024).toFixed(1)} MB`;
  },
  tokens(n) {
    if (!Number.isFinite(n)) return '—';
    if (n < 1000) return String(n);
    if (n < 1e6) return `${(n / 1000).toFixed(n < 1e4 ? 1 : 0)}k`;
    return `${(n / 1e6).toFixed(n < 1e7 ? 2 : 1)}M`;
  },
  lines(text) {
    if (!text) return 0;
    return text.split('\n').length - (text.endsWith('\n') ? 1 : 0);
  },
});

// kv is a key · · · value row, as the inspector shows them.
export function kv(key, value, cls) {
  return h('div', { class: `kv${cls ? ` ${cls}` : ''}` }, h('span', { class: 'k', text: key }), h('span', { class: 'dots' }), h('span', { class: 'v', text: value }));
}

export function uuid() {
  if (crypto.randomUUID) return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 15) | 64;
  b[8] = (b[8] & 63) | 128;
  const x = [...b].map((v) => v.toString(16).padStart(2, '0')).join('');
  return `${x.slice(0, 8)}-${x.slice(8, 12)}-${x.slice(12, 16)}-${x.slice(16, 20)}-${x.slice(20)}`;
}

// typingIn reports whether keys pressed in target type text.
export function typingIn(target) {
  return target instanceof HTMLElement && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));
}
