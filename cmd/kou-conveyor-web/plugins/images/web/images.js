// images: images in prompts (attach.js). The composer's strip goes in
// composer.above, the picture shown large in stage.overlay, a sent prompt's
// images under it in the timeline, and the picture a tool call read (a
// ViewImage call's) under the call. It provides the images service:
// take(text), set(list), load(infos, urlOf), encode(list), info(att),
// remember(messageID, list), imageURL(v, entry, n), queuedImageURL(v, item, n),
// sessionImageURL(ws, session, message, n), openImage(v, entry, n),
// toolImageURL(v, entry), openToolImage(v, entry), attach(options) for other
// textareas, and the drafts' saveDraft(key), loadDraft(key), dropDraft(key).
import { createAttachments, createPreview, encodeImages, info, promptImages, toolImage } from './attach.js';

export default function activate(cockpit) {
  const { h } = cockpit;
  const composer = cockpit.use('composer');
  const hot = cockpit.hot.data;
  // Kept across versions: drafts' images by draft key, those this tab sent
  // by message ID (the transcript shows them from memory until the server
  // has them), and the composer's.
  hot.drafts ??= new Map();
  hot.sent ??= new Map();
  hot.composer ??= [];

  const previewNode = h('div', { class: 'image-preview', id: 'image-preview', hidden: true, 'aria-hidden': 'true' });
  cockpit.ui.mount('stage.overlay', { id: 'image-preview', order: 0, node: previewNode });
  const preview = createPreview(previewNode, () => cockpit.use('timeline').scroller?.() || document.getElementById('scroll'));
  cockpit.onDispose(() => preview.destroy());

  const strip = h('div', { class: 'attachments', id: 'attachments', hidden: true });
  cockpit.ui.mount('composer.above', { id: 'attachments', order: 20, node: strip });
  let attached = null;

  // attachComposer follows the composer's textarea: the composer loaded
  // anew has a new one.
  function attachComposer() {
    const input = composer.input;
    if (!input || attached?.input === input) return;
    const list = attached?.ctl.list || hot.composer;
    attached?.ctl.destroy();
    const ctl = createAttachments({
      input, strip, preview, drop: composer.form || input, toast: (text, kind) => cockpit.toast(text, kind),
      onChange: () => {
        composer.autosize?.();
        cockpit.render();
      },
    });
    ctl.set(list);
    attached = { input, ctl };
  }
  cockpit.on('composer:ready', attachComposer);
  cockpit.on('service', (name) => { if (name === 'composer') attachComposer(); });
  attachComposer();
  cockpit.onDispose(() => {
    hot.composer = attached?.ctl.list || [];
    attached?.ctl.destroy();
  });
  const ctl = () => attached?.ctl || null;

  function remember(messageID, list) {
    if (list?.length) hot.sent.set(messageID, list);
  }

  function sessionImageURL(ws, sessionID, messageID, n) {
    return cockpit.wsPath(ws, `/sessions/${encodeURIComponent(sessionID)}/images/${encodeURIComponent(messageID)}/${n}`);
  }

  function queuedImageURL(v, item, n) {
    const local = hot.sent.get(item.id)?.find((att) => att.label === item.images[n]?.label);
    return local?.url || cockpit.wsPath(v.ws, `/sessions/${encodeURIComponent(v.id)}/queue/${encodeURIComponent(item.id)}/images/${n}`);
  }

  // imageURL is where image n of a prompt shows from: this tab's copy of an
  // image it sent, else the session.
  function imageURL(v, entry, n) {
    const messageID = entry.id.replace(/^input:/, '');
    const local = hot.sent.get(messageID)?.find((att) => att.label === entry.images?.[n]?.label);
    return local?.url || sessionImageURL(v.ws, v.id, messageID, n);
  }

  function openImage(v, entry, n) {
    const image = entry.images?.[n];
    if (image) preview.open({ ...image, type: image.media_type, url: imageURL(v, entry, n) });
  }

  cockpit.contribute('timeline.decoration', {
    kinds: ['user'], order: 10,
    render: (entry, ctx) => promptImages(entry, (n) => imageURL(ctx.view, entry, n), (n) => openImage(ctx.view, entry, n)),
  });

  // toolImageURL is where the picture a tool call read shows from: the
  // session keeps it with the call.
  function toolImageURL(v, entry) {
    const call = entry.tool?.image && entry.tool.call_id;
    return call ? cockpit.wsPath(v.ws, `/sessions/${encodeURIComponent(v.id)}/tools/${encodeURIComponent(call)}/image`) : '';
  }

  function openToolImage(v, entry) {
    const image = entry.tool?.image;
    if (image) preview.open({ ...image, type: image.media_type, url: toolImageURL(v, entry) });
  }

  cockpit.contribute('timeline.decoration', {
    kinds: ['tool'], order: 10,
    render: (entry, ctx) => toolImage(entry, toolImageURL(ctx.view, entry), () => openToolImage(ctx.view, entry), ctx.expanded(entry)),
  });

  cockpit.provide('images', {
    take: (text) => ctl()?.take(text) || [],
    set: (list) => ctl()?.set(list),
    load: (infos, urlOf) => ctl()?.load(infos, urlOf),
    list: () => ctl()?.list || [],
    info, encode: encodeImages, remember, imageURL, queuedImageURL, sessionImageURL, openImage, toolImageURL, openToolImage,
    saveDraft: (key) => hot.drafts.set(key, ctl()?.list || []),
    loadDraft: (key) => ctl()?.set(hot.drafts.get(key)),
    dropDraft: (key) => hot.drafts.delete(key),
    // attach gives another textarea, such as a prompt being edited, images
    // of its own; destroy() lets go of it.
    attach: (options) => createAttachments({ preview, toast: (text, kind) => cockpit.toast(text, kind), ...options }),
    preview: () => preview,
  });

  cockpit.contribute('help.keys', { keys: ['⌘', 'V'], text: 'Paste an image (or drop one): the prompt names it [Image 1] · with the caret right after the label it shows large · ⌫ there removes it', order: 45 });
}
