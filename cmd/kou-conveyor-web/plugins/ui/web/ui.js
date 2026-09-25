// ui: what plugins show things with. It provides the services
//
//   toast       show(message, kind, key), dismiss(key): short messages; a
//               later one with the same key replaces the earlier
//   clipboard   copy(text, label)
//   menu        open(anchor, items), close(), toggle(anchor, items),
//               anchor(): a menu of { icon, label, detail, hint, danger,
//               confirm, disabled, run } or { separator: true }
//   overlays    closeTop(), closeAll(), open(): the dialogs and floating
//               things plugins contribute to "overlay"
//
// The contribution point "overlay" takes { id, order, modal, isOpen(),
// close() }: Esc closes the first open one by order, and while a modal one
// is open, keys that are not global do nothing.
export default function activate(cockpit) {
  const { h } = cockpit;

  // ---------------------------------------------------------------- toasts

  const toasts = h('div', { class: 'toasts', id: 'toasts', 'aria-live': 'polite' });
  cockpit.ui.mount('overlays', { id: 'toasts', order: 100, node: toasts });

  function show(message, kind = 'info', key = '') {
    const el = h('div', { class: `toast ${kind}`, role: kind === 'error' ? 'alert' : 'status', data: { key: key || null } }, h('span', { text: message }));
    if (key) toasts.querySelector(`[data-key="${CSS.escape(key)}"]`)?.remove();
    toasts.append(el);
    while (toasts.children.length > 3) toasts.firstChild.remove();
    setTimeout(() => {
      el.classList.add('out');
      setTimeout(() => el.remove(), 220);
    }, kind === 'error' ? 6000 : 2800);
  }
  function dismiss(key) {
    toasts.querySelector(`[data-key="${CSS.escape(key)}"]`)?.remove();
  }
  cockpit.provide('toast', { show, dismiss });

  // ---------------------------------------------------------------- clipboard

  async function copy(text, label = 'Copied') {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // The Clipboard API needs a secure context; plain-HTTP hosts fall back.
      const area = h('textarea', { class: 'clip', readonly: true });
      area.value = text;
      document.body.append(area);
      area.select();
      const ok = document.execCommand('copy');
      area.remove();
      if (!ok) return show('Copy failed', 'error');
    }
    show(label);
  }
  cockpit.provide('clipboard', { copy });

  // ---------------------------------------------------------------- menu

  const menu = h('div', { class: 'menu', id: 'menu', role: 'menu', hidden: true });
  cockpit.ui.mount('overlays', { id: 'menu', order: 90, node: menu });
  let anchor = null;

  function open(at, items) {
    close();
    menu.replaceChildren(...items.filter(Boolean).map((item) => {
      if (item.separator) return h('div', { class: 'sep', role: 'separator' });
      const label = h('span', { class: 'l', text: item.label });
      const text = item.detail ? h('span', { class: 'lines' }, label, h('small', { text: item.detail })) : label;
      return h('button', {
        type: 'button', role: 'menuitem', class: item.danger ? 'danger' : null, disabled: !!item.disabled,
        onclick: (event) => {
          event.stopPropagation();
          // Destructive items take a second, deliberate click.
          if (item.confirm && event.currentTarget.dataset.armed !== 'true') {
            event.currentTarget.dataset.armed = 'true';
            label.textContent = item.confirm;
            return;
          }
          close();
          cockpit.safely(item.run);
        },
      }, h('span', { class: 'i', 'aria-hidden': 'true', text: item.icon || '' }), text, item.hint ? h('kbd', { text: item.hint }) : null);
    }));
    menu.hidden = false;
    anchor = at;
    at.setAttribute('aria-expanded', 'true');
    const box = at.getBoundingClientRect();
    const width = menu.offsetWidth;
    const height = menu.offsetHeight;
    menu.style.left = `${Math.max(8, Math.min(box.right - width, window.innerWidth - width - 8))}px`;
    menu.style.top = `${box.bottom + height + 8 > window.innerHeight ? Math.max(8, box.top - height - 4) : box.bottom + 4}px`;
    menu.querySelector('button:not(:disabled)')?.focus();
  }

  function close() {
    if (menu.hidden) return false;
    menu.hidden = true;
    anchor?.setAttribute('aria-expanded', 'false');
    if (document.activeElement?.closest?.('#menu')) anchor?.focus();
    anchor = null;
    return true;
  }

  // toggle opens the menu by anchor, or closes it if it is open there.
  function toggle(at, items) {
    if (anchor === at) {
      close();
      return;
    }
    open(at, typeof items === 'function' ? items() : items);
  }

  cockpit.provide('menu', { open, close, toggle, anchor: () => anchor, isOpen: () => !menu.hidden });
  cockpit.listen(document, 'mousedown', (event) => {
    if (!menu.hidden && !event.target.closest('#menu') && event.target.closest('button') !== anchor) close();
  });
  cockpit.listen(window, 'resize', () => close());
  cockpit.keys.register({
    key: ['ArrowDown', 'ArrowUp'], global: true, priority: 100,
    when: () => !menu.hidden,
    run: (event) => {
      const items = [...menu.querySelectorAll('button:not(:disabled)')];
      const at = items.indexOf(document.activeElement);
      items[(at + (event.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length]?.focus();
    },
  });
  cockpit.contribute('overlay', { id: 'menu', order: 20, modal: false, isOpen: () => !menu.hidden, close });

  // ---------------------------------------------------------------- overlays

  const overlays = () => cockpit.contributions('overlay', { unique: 'id' });
  const isOpen = (o) => !!cockpit.safely(() => o.isOpen());
  function closeTop() {
    const top = overlays().find(isOpen);
    if (!top) return false;
    cockpit.safely(() => top.close());
    return true;
  }
  function closeAll() {
    let closed = false;
    for (const o of overlays()) {
      if (isOpen(o)) {
        cockpit.safely(() => o.close());
        closed = true;
      }
    }
    return closed;
  }
  cockpit.provide('overlays', { closeTop, closeAll, open: () => overlays().filter(isOpen).map((o) => o.id) });
  cockpit.keys.register({ key: 'Escape', global: true, priority: 100, run: () => (closeTop() ? undefined : false) });
  cockpit.keys.guard(() => overlays().some((o) => o.modal && isOpen(o)));
}
