// What every plugin's script finds in its world: `kou`, to say what it adds
// to the app, and `ui`, to build what it draws. The app talks to this file
// through `__kou`, in JSON both ways: a script's functions never leave it.
(function (global) {
  "use strict";

  // The pages the plugin adds, by id, and the handlers of each page as it
  // was last drawn.
  const pages = new Map();
  const drawn = new Map();
  // The colours and sizes of the app, as the page being drawn is given them.
  let theme = {};

  function channels(color) {
    let hex = String(color);
    if (!hex.startsWith("#")) hex = String(theme[hex] ?? "#000000");
    hex = hex.slice(1);
    if (hex.length === 3) hex = hex.replace(/./g, "$&$&");
    const part = (at) => parseInt(hex.slice(at, at + 2), 16) || 0;
    const alpha = hex.length >= 8 ? part(6) / 255 : 1;
    return [part(0), part(2), part(4), alpha];
  }

  function hex(r, g, b, a) {
    const two = (value) =>
      Math.max(0, Math.min(255, Math.round(value))).toString(16).padStart(2, "0");
    return "#" + two(r) + two(g) + two(b) + (a < 1 ? two(a * 255) : "");
  }

  function flat(children, out) {
    if (Array.isArray(children)) {
      for (const child of children) flat(child, out);
    } else if (children !== null && children !== undefined && children !== false && children !== true) {
      out.push(children);
    }
    return out;
  }

  const ui = {
    // An element of a kind the app draws, with what it is given and holds.
    el(type, props, children) {
      return { ...(props || {}), type, children: flat(children, []) };
    },
    box: (props, children) => ui.el("box", props, children),
    row: (props, children) => ui.el("box", { dir: "row", ...(props || {}) }, children),
    col: (props, children) => ui.el("box", { dir: "col", ...(props || {}) }, children),
    text: (value, props) => ui.el("text", { ...(props || {}), value: String(value) }),
    icon: (name, props) => ui.el("icon", { ...(props || {}), name }),
    button: (label, props) => ui.el("button", { ...(props || {}), label: String(label) }),
    iconButton: (name, props) => ui.el("iconButton", { ...(props || {}), name }),
    // A part that stays the app's own to draw, placed here: which ones
    // there are is said where a view is given them.
    slot: (name) => ui.el("slot", { name: String(name) }),
    monogram: (code, props) => ui.el("monogram", { ...(props || {}), code: String(code) }),
    // A harness's mark (by its name, or its `code`), in its maker's colour.
    brand: (harness, props) => ui.el("brand", { ...(props || {}), harness: String(harness) }),
    mark: (state, props) => ui.el("mark", { ...(props || {}), state: String(state) }),
    // A colour (a name of the theme's, or `#rrggbb`) at `share` of its strength.
    alpha(color, share) {
      const [r, g, b, a] = channels(color);
      return hex(r, g, b, a * share);
    },
    // `over` laid on `under` at `share` of its strength, as one opaque colour.
    mix(under, over, share) {
      const [a, b] = [channels(under), channels(over)];
      return hex(
        a[0] + (b[0] - a[0]) * share,
        a[1] + (b[1] - a[1]) * share,
        a[2] + (b[2] - a[2]) * share,
        1,
      );
    },
  };

  const kou = {
    // Adds a page: `{ id, title, icon, render(ctx) }`. A page of an id that
    // another plugin has takes its place when this plugin comes later.
    page(page) {
      if (!page || typeof page.id !== "string" || typeof page.render !== "function") {
        throw new TypeError("kou.page takes { id, render(ctx) }");
      }
      pages.set(page.id, { ...page, kind: "page" });
    },
    // Adds a view: `{ id, render(ctx) }`, something the app draws where it
    // has a place for it (a surface in a chat, say), as many times as it
    // has things to show: each is drawn from its own data and keeps its
    // own state.
    view(view) {
      if (!view || typeof view.id !== "string" || typeof view.render !== "function") {
        throw new TypeError("kou.view takes { id, render(ctx) }");
      }
      pages.set(view.id, { ...view, kind: "view" });
    },
    get theme() {
      return theme;
    },
  };

  // What was built, as the app takes it: text as elements, nothing empty,
  // and each function kept here under a number the app calls it back by.
  function seal(node, handlers) {
    if (typeof node === "string" || typeof node === "number") {
      return { type: "text", value: String(node), children: [] };
    }
    if (node === null || typeof node !== "object") return null;
    const out = {};
    for (const [key, value] of Object.entries(node)) {
      if (key === "children") continue;
      if (typeof value === "function") {
        handlers.push(value);
        out[key] = { $h: handlers.length - 1 };
      } else if (value !== undefined) {
        out[key] = value;
      }
    }
    out.type = typeof node.type === "string" ? node.type : "box";
    out.children = flat(node.children, [])
      .map((child) => seal(child, handlers))
      .filter((child) => child !== null);
    return out;
  }

  global.kou = kou;
  global.ui = ui;
  global.__kou = {
    pages() {
      return JSON.stringify(
        [...pages.values()].map((page) => ({
          id: page.id,
          title: typeof page.title === "string" ? page.title : page.id,
          icon: typeof page.icon === "string" ? page.icon : null,
          page: page.kind === "page",
        })),
      );
    },
    // `instance` tells apart the things one view is drawn for.
    render(id, input, state, instance) {
      const page = pages.get(id);
      if (!page) throw new Error('no page "' + id + '"');
      const given = JSON.parse(input);
      theme = given.theme || {};
      const handlers = [];
      const built = page.render({
        data: given.data,
        theme,
        state: JSON.parse(state),
      });
      const tree = seal(built, handlers) || { type: "box", children: [] };
      // (The handlers of a few hundred drawings are kept, the oldest let go.)
      const key = id + "\n" + (instance || "");
      drawn.delete(key);
      drawn.set(key, handlers);
      // More than the app keeps trees for (`DRAWN` in host.rs): a tree it
      // still holds must find its handlers here, or its clicks do nothing.
      if (drawn.size > 1024) drawn.delete(drawn.keys().next().value);
      return JSON.stringify(tree);
    },
    dispatch(id, handler, event, state, instance) {
      const handlers = drawn.get(id + "\n" + (instance || ""));
      const run = handlers && handlers[handler];
      const told = { acts: [], state: null };
      if (typeof run !== "function") return JSON.stringify(told);
      let kept = JSON.parse(state);
      run(
        {
          get state() {
            return kept;
          },
          // Keeps what the page remembers; it is drawn again with it.
          setState(patch) {
            kept = { ...(kept || {}), ...(patch || {}) };
            told.state = kept;
          },
          // Asks the app to do something of its own: `usage.refresh`, ...
          act(name, args) {
            told.acts.push([String(name), args === undefined ? null : args]);
          },
        },
        JSON.parse(event),
      );
      return JSON.stringify(told);
    },
  };
})(globalThis);
