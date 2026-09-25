// Images in prompts. ⌘V or Ctrl+V (or dropping files) puts an image in a
// prompt: its text names the image by a label, [Image 1] for the first, and
// the prompt brings the images its text still names. A strip above the text
// shows them small. With the caret right after a label, the image shows
// large over the transcript while typing goes on; Backspace there takes the
// whole label out, and its image with it. The images of sent prompts show
// under them, and open large on a click; so do the pictures the agent looked
// at, under the calls that read them.

import { fmt, h } from '/kernel/dom.js';

// The formats the server takes; it sends PNG, JPEG, GIF and WebP on as they
// are and converts the rest.
const TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/bmp', 'image/tiff'];
export const MAX_IMAGES = 20;
const MAX_BYTES = 64 * 1024 * 1024;
const LABEL = /\[Image ([1-9][0-9]{0,5})\]/g;

export const imageLabel = (n) => `[Image ${n}]`;

// describe says what an image is: 1920×1080 PNG · 240 KB.
export function describe(image) {
  const kind = (image.media_type || image.type || '').replace(/^image\//, '').toUpperCase();
  const size = [image.width ? `${image.width}×${image.height}` : '', kind].filter(Boolean).join(' ');
  return [size, image.size ? fmt.bytes(image.size) : ''].filter(Boolean).join(' · ');
}

// An attachment is { label, url, blob, type, size, width, height, ready }:
// ready settles once its bytes and size are known.
function fromBlob(blob, label) {
  const att = { label, blob, url: URL.createObjectURL(blob), type: blob.type, size: blob.size, width: 0, height: 0 };
  att.ready = measure(att);
  return att;
}

// fromURL is an image a sent prompt brought, to edit the prompt with.
function fromURL(info, url) {
  const att = { label: info.label, url, blob: null, type: info.media_type, size: info.size, width: info.width, height: info.height };
  att.ready = fetch(url)
    .then((res) => (res.ok ? res.blob() : Promise.reject(new Error(`${info.label} is no longer available`))))
    .then((blob) => { att.blob = blob; })
    .catch(() => { att.broken = true; });
  return att;
}

async function measure(att) {
  const img = new Image();
  img.src = att.url;
  try {
    await img.decode();
    att.width = img.naturalWidth;
    att.height = img.naturalHeight;
  } catch {
    att.broken = true; // BMP and TIFF the browser may not draw; the server reads them
  }
}

// encodeImages is what a request brings of attachments: base64 bytes.
export async function encodeImages(list) {
  const out = [];
  for (const att of list) {
    await att.ready;
    if (!att.blob) throw new Error(`${att.label} could not be read`);
    out.push({ label: att.label, media_type: att.blob.type || att.type || '', data: await base64(att.blob) });
  }
  return out;
}

function base64(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(',')[1] || '');
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
}

// info describes an attachment the way the server describes images.
export function info(att) {
  return { label: att.label, media_type: att.blob?.type || att.type, width: att.width, height: att.height, size: att.blob?.size || att.size };
}

// imageFiles are the images a paste or a drop brings.
function imageFiles(data) {
  const files = [...(data?.files || [])].filter((file) => file.type.startsWith('image/'));
  if (!files.length) {
    for (const item of data?.items || []) {
      if (item.kind === 'file' && item.type.startsWith('image/')) {
        const file = item.getAsFile();
        if (file) files.push(file);
      }
    }
  }
  return files;
}

// edit replaces text in a textarea the way typing does, so undo takes it
// back, and tells the page it changed.
function edit(input, start, end, text) {
  input.focus({ preventScroll: true });
  input.setSelectionRange(start, end);
  const done = text ? document.execCommand('insertText', false, text) : document.execCommand('delete');
  if (!done) {
    input.setRangeText(text, start, end, 'end');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  }
}

// setData sets a data attribute, or takes it away for null, unless it is
// so already, and says whether it changed it.
function setData(node, key, value) {
  if ((node.dataset[key] ?? null) === value) return false;
  if (value == null) delete node.dataset[key];
  else node.dataset[key] = value;
  return true;
}

// place makes box's children the nodes given, moving as few as it can — a
// node put in again plays its entrance again — and says whether it moved
// any.
function place(box, nodes) {
  const want = new Set(nodes);
  let moved = false;
  for (const child of [...box.childNodes]) {
    if (!want.has(child)) {
      child.remove();
      moved = true;
    }
  }
  let at = box.firstChild;
  for (const node of nodes) {
    if (node === at) at = at.nextSibling;
    else {
      box.insertBefore(node, at);
      moved = true;
    }
  }
  return moved;
}

// ---------------------------------------------------------------- attachments

// createAttachments keeps the images of a textarea's text: the composer's,
// or a prompt's being edited. strip is where they show small (one is made
// when none is given), preview where the one at the caret shows large, and
// onChange hears when what the strip shows changes.
export function createAttachments({ input, strip, preview, drop = input, toast = () => {}, onChange = () => {} }) {
  strip ??= h('div', { class: 'attachments', hidden: true });
  let list = [];
  let active = null;
  // Each image keeps its card while it is in the list, so typing leaves the
  // strip as it is: drawn anew at every key, the cards played their
  // entrance again and loaded their pictures anew, and shook as one typed.
  const cards = new Map(); // attachment → { node, caption }
  const hint = h('span', { class: 'attachments-hint', text: 'The caret right after a label shows it large · ⌫ there removes it' });

  const ctl = {
    strip,
    get list() { return list; },
    // referenced are the images text names, each once.
    referenced(text = input.value) {
      const seen = new Set();
      return list.filter((att) => text.includes(att.label) && !seen.has(att.label) && seen.add(att.label)).slice(0, MAX_IMAGES);
    },
    // take hands the images text names over to a prompt, and empties the
    // list: they go with it.
    take(text = input.value) {
      const sent = ctl.referenced(text);
      list = [];
      render();
      return sent;
    },
    set(items) {
      list = [...(items || [])];
      render();
    },
    clear() { ctl.set([]); },
    // load makes the images a sent prompt brought this text's again.
    load(infos, urlOf) {
      ctl.set((infos || []).map((one, n) => fromURL(one, urlOf(n))));
      for (const att of list) att.ready.then(render);
    },
    add,
    render,
    sync,
  };

  async function add(files) {
    const room = MAX_IMAGES - ctl.referenced().length;
    if (files.length > room) toast(`A prompt takes at most ${MAX_IMAGES} images`, 'warn');
    const labels = [];
    for (const file of files.slice(0, Math.max(0, room))) {
      if (!TYPES.includes(file.type)) {
        toast(`${file.name || 'The image'} is ${file.type || 'of an unknown type'}: PNG, JPEG, GIF, WebP, BMP and TIFF go`, 'error');
        continue;
      }
      if (file.size > MAX_BYTES) {
        toast(`${file.name || 'The image'} is ${fmt.bytes(file.size)}: images of up to 64 MB go`, 'error');
        continue;
      }
      const numbers = [...input.value.matchAll(LABEL)].map((m) => Number(m[1]));
      const label = imageLabel(Math.max(0, ...numbers, ...list.map((att) => Number(att.label.match(/\d+/)?.[0] || 0))) + 1);
      const att = fromBlob(file, label);
      list.push(att);
      att.ready.then(render);
      // Several go in one after another, a space apart.
      const at = input.selectionStart;
      const before = input.value.slice(0, at);
      const text = (labels.length && !/\s$/.test(before) ? ' ' : '') + label;
      edit(input, at, input.selectionEnd, text);
      labels.push(label);
    }
    render();
    sync();
    return labels;
  }

  // atCaret is the image whose label ends right at the caret.
  function atCaret() {
    if (document.activeElement !== input || input.selectionStart !== input.selectionEnd) return null;
    const at = input.selectionStart;
    return list.find((att) => at >= att.label.length && input.value.slice(at - att.label.length, at) === att.label) || null;
  }

  // sync shows the image at the caret large, and marks it in the strip.
  function sync() {
    const att = atCaret();
    if (att !== active) {
      active = att;
      for (const [one, card] of cards) card.node.dataset.active = one === att ? 'true' : 'false';
    }
    // The text being edited stays in sight: an editor in the transcript
    // keeps the picture off itself.
    if (att) preview?.show(att, ctl, input.closest('.edit') || input);
    else preview?.hide(ctl);
  }

  // render shows the images the text names. It changes only what differs
  // from what the strip shows, and tells onChange only when it changed
  // something: most keys change nothing there.
  function render() {
    for (const att of cards.keys()) if (!list.includes(att)) cards.delete(att);
    const shown = ctl.referenced();
    let changed = strip.hidden !== !shown.length;
    if (changed) strip.hidden = !shown.length;
    const nodes = [];
    for (const att of shown) {
      const [node, updated] = card(att);
      nodes.push(node);
      changed = updated || changed;
    }
    if (nodes.length) nodes.push(hint);
    changed = place(strip, nodes) || changed;
    if (changed) onChange();
  }

  // card is an image's figure in the strip, made when it first shows and
  // brought up to date after; updated says whether that changed it.
  function card(att) {
    let c = cards.get(att);
    const made = !c;
    if (made) {
      c = { caption: h('span') };
      c.node = h('figure', { class: 'attachment', data: { label: att.label, active: att === active ? 'true' : 'false' } },
        h('button', {
          class: 'attachment-thumb', type: 'button', title: `Show ${att.label} large: the caret goes right after its label`,
          onclick: () => caretAfter(att.label),
        }, h('img', { src: att.url, alt: att.label, draggable: 'false' })),
        h('figcaption', null, h('b', { text: att.label }), c.caption),
        h('button', {
          class: 'attachment-drop', type: 'button', text: '×', 'aria-label': `Remove ${att.label}`, title: 'Remove the image and its label',
          onclick: () => removeLabel(att.label),
        }));
      cards.set(att, c);
    }
    const caption = describe(info(att)) || 'Reading…';
    const described = c.caption.textContent !== caption;
    if (described) c.caption.textContent = caption;
    const broken = setData(c.node, 'broken', att.broken ? 'true' : null);
    return [c.node, made || described || broken];
  }

  function caretAfter(label) {
    const at = input.value.indexOf(label);
    if (at < 0) return;
    input.focus();
    input.setSelectionRange(at + label.length, at + label.length);
    sync();
  }

  function removeLabel(label) {
    for (let at = input.value.lastIndexOf(label); at >= 0; at = input.value.lastIndexOf(label)) {
      edit(input, at, at + label.length, '');
    }
    render();
    sync();
  }

  const listeners = [];
  const on = (target, type, fn, options) => {
    target.addEventListener(type, fn, options);
    listeners.push(() => target.removeEventListener(type, fn, options));
  };
  on(input, 'paste', (event) => {
    const files = imageFiles(event.clipboardData);
    if (!files.length) return; // text pastes as ever
    // Office apps put a picture of what was copied beside its text: the
    // text is what was meant, unless it only names the files.
    const text = event.clipboardData.getData('text/plain').trim();
    if (text && !files.every((file) => file.name && text.includes(file.name))) return;
    event.preventDefault();
    add(files);
  });
  on(input, 'keydown', (event) => {
    // Backspace right after a label takes it out at once.
    if (event.key !== 'Backspace' || event.altKey || event.metaKey || event.ctrlKey || event.isComposing) return;
    const att = atCaret();
    if (!att) return;
    event.preventDefault();
    const at = input.selectionStart;
    edit(input, at - att.label.length, at, '');
    render();
    sync();
  });
  on(drop, 'dragover', (event) => {
    if (![...(event.dataTransfer?.items || [])].some((item) => item.kind === 'file')) return;
    event.preventDefault();
    drop.dataset.dropping = 'true';
  });
  on(drop, 'dragleave', (event) => {
    if (!drop.contains(event.relatedTarget)) delete drop.dataset.dropping;
  });
  on(drop, 'drop', (event) => {
    delete drop.dataset.dropping;
    const files = imageFiles(event.dataTransfer);
    if (!files.length) return;
    event.preventDefault();
    input.focus();
    add(files);
  });
  // Whatever moves the caret: typing, arrows, clicks, the selection.
  const later = () => requestAnimationFrame(sync);
  for (const type of ['input', 'keyup', 'click', 'select', 'focus', 'blur']) on(input, type, later);
  on(input, 'input', () => render());
  // An editor's textarea leaves the page when the edit ends, and its
  // listener with it.
  let attached = false;
  const selection = () => {
    if (input.isConnected) attached = true;
    else if (attached) {
      document.removeEventListener('selectionchange', selection);
      preview?.hide(ctl);
      return;
    }
    later();
  };
  document.addEventListener('selectionchange', selection);
  // destroy lets go of the textarea: a plugin loaded anew attaches again.
  ctl.destroy = () => {
    for (const off of listeners.splice(0)) off();
    document.removeEventListener('selectionchange', selection);
    preview?.hide(ctl);
  };
  return ctl;
}

// ---------------------------------------------------------------- preview

// createPreview shows an image large over the transcript (area): for the
// label the caret is at, without taking the keys or the pointer, or opened
// on a click, when a click anywhere or Esc closes it.
export function createPreview(node, area) {
  const img = h('img', { alt: '' });
  const label = h('b');
  const what = h('span');
  const hint = h('span', { class: 'image-preview-hint' });
  node.replaceChildren(h('figure', { class: 'image-preview-card ticks' },
    h('div', { class: 'image-preview-frame' }, img),
    h('figcaption', null, label, what, hint)));
  let owner = null;
  let pinned = false;
  let avoid = null;

  // place fits the picture to the transcript, less what it must not cover:
  // it takes the larger side of that, when there is room there.
  function place() {
    const scroller = area();
    if (!scroller) return;
    const box = scroller.getBoundingClientRect();
    let top = box.top;
    let bottom = box.bottom;
    const keep = avoid?.getBoundingClientRect();
    if (keep && keep.bottom > box.top && keep.top < box.bottom) {
      const above = keep.top - box.top;
      const below = box.bottom - keep.bottom;
      if (Math.max(above, below) >= 120) {
        if (above >= below) bottom = keep.top - 6;
        else top = keep.bottom + 6;
      }
    }
    Object.assign(node.style, { top: `${top}px`, left: `${box.left}px`, width: `${box.width}px`, height: `${Math.max(0, bottom - top)}px` });
  }

  function open(att, by, sticky, keepClear = null) {
    avoid = keepClear;
    place();
    if (img.getAttribute('src') !== att.url) img.src = att.url;
    img.alt = att.label;
    label.textContent = att.label;
    what.textContent = describe(info(att));
    hint.textContent = sticky ? 'Click or Esc closes it' : '⌫ removes it · the caret elsewhere closes it';
    owner = by;
    pinned = sticky;
    node.dataset.sticky = sticky ? 'true' : 'false';
    node.hidden = false;
    requestAnimationFrame(() => { node.dataset.open = 'true'; });
  }

  function close() {
    owner = null;
    pinned = false;
    delete node.dataset.open;
    node.hidden = true;
  }

  node.addEventListener('click', () => { if (pinned) close(); });
  const onKey = (event) => {
    if (pinned && event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation(); // not the first Esc of an Esc Esc
      close();
    }
  };
  const onResize = () => { if (!node.hidden) place(); };
  const onScroll = (event) => { if (!node.hidden && event.target === area()) place(); };
  document.addEventListener('keydown', onKey, true);
  window.addEventListener('resize', onResize);
  document.addEventListener('scroll', onScroll, { capture: true, passive: true });

  return {
    show(att, by, keepClear) { if (!pinned) open(att, by, false, keepClear); },
    hide(by) { if (!pinned && owner === by) close(); },
    open(att) { open(att, null, true); },
    close,
    isOpen: () => !node.hidden,
    destroy() {
      document.removeEventListener('keydown', onKey, true);
      window.removeEventListener('resize', onResize);
      document.removeEventListener('scroll', onScroll, { capture: true });
      close();
    },
  };
}

// ---------------------------------------------------------------- sent prompts

// ---------------------------------------------------------------- what the agent looked at

// fitWithin is the size of a width×height picture in at most max×max
// pixels, in proportion, and never larger than it is; [0, 0] when its size
// is not known.
function fitWithin(width, height, maxWidth, maxHeight) {
  if (!(width > 0 && height > 0)) return [0, 0];
  const scale = Math.min(1, maxWidth / width, maxHeight / height);
  return [Math.max(1, Math.round(width * scale)), Math.max(1, Math.round(height * scale))];
}

// toolImage renders the picture a tool call read (a ViewImage call's, as the
// model got it) under the call: small while the call is folded, larger
// while it is open. A click opens it large. Its size is known before it
// loads, so nothing below it moves when it does.
export function toolImage(entry, url, onOpen, open = false) {
  const image = entry.tool?.image;
  if (!image || !url) return null;
  const [width, height] = open ? fitWithin(image.width, image.height, 720, 480) : fitWithin(image.width, image.height, 280, 160);
  const what = describe(image);
  const box = h('div', { class: 'tool-image', data: { open: open ? 'true' : null } });
  box.append(
    h('button', {
      class: 'tool-image-frame', type: 'button', title: `${image.label} · ${what} — the picture as the model got it; click to see it large`,
      onclick: (event) => {
        event.stopPropagation();
        onOpen();
      },
    }, h('img', {
      src: url, alt: image.label || 'The picture the agent looked at', width: width || null, height: height || null,
      loading: 'lazy', draggable: 'false',
      // One the session file is still taking in comes a moment later; one
      // a rewind took away does not.
      onerror: (event) => {
        const el = event.currentTarget;
        if (el.dataset.retried) {
          box.dataset.broken = 'true';
          return;
        }
        el.dataset.retried = 'true';
        setTimeout(() => { el.src = `${url}${url.includes('?') ? '&' : '?'}retry=1`; }, 900);
      },
    })),
    h('span', { class: 'tool-image-what', text: what }));
  return box;
}

// promptImages renders the images a sent prompt brought, small, under it;
// a click opens one large.
export function promptImages(entry, urlOf, onOpen) {
  if (!entry.images?.length) return null;
  return h('div', { class: 'prompt-images' }, entry.images.map((image, n) => h('button', {
    class: 'prompt-image', type: 'button', title: `${image.label} · ${describe(image)} — click to see it large`,
    onclick: () => onOpen(n),
  }, h('img', {
    src: urlOf(n), alt: image.label, loading: 'lazy', draggable: 'false',
    // A prompt the runner is still recording has no image to serve yet.
    onerror: (event) => {
      const el = event.currentTarget;
      if (el.dataset.retried || el.src.startsWith('blob:')) return;
      el.dataset.retried = 'true';
      setTimeout(() => { el.src = `${urlOf(n)}${urlOf(n).includes('?') ? '&' : '?'}retry=1`; }, 900);
    },
  }), h('span', { text: image.label }))));
}
