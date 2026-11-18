// The canvas's chrome: the bar in the board's corner, what an empty board
// offers, the open menu, and the bar that takes a deletion back. How they
// look and how they are laid out is all here; what is in them is the
// canvas's, in `ctx.data`, and what they do is asked of it with
// `api.act("canvas.<name>")`. The board itself (its nodes, their ports, the
// edges and their labels, the grid) stays the canvas's own, and so does what
// moves with each frame: the zoom's percentage (`ui.slot("percent")`), the
// menu's travelling highlight (`ui.slot("highlight")`), and the fading in
// and out of the menu and the bar after a deletion.

function keyHint(keys, m) {
  return ui.text(keys, {
    shrink: 0,
    px: 4,
    radius: m.radiusSmall,
    border: 1,
    borderColor: "lineStrong",
    font: "mono",
    size: 10.5,
    color: "fgMuted",
  });
}

// The room between a menu's or the bar's edge and what is in it.
const PAD = 4;

// The bar in the board's corner: add, zoom, fit. Acts: canvas.add,
// canvas.zoomOut, canvas.zoomMenu, canvas.zoomIn, canvas.fit.
kou.view({
  id: "canvasBar",
  render(ctx) {
    const m = ctx.theme.metric;
    return ui.row(
      {
        position: "absolute",
        left: 12,
        bottom: 12,
        h: 36,
        px: PAD,
        items: "center",
        gap: 2,
        radius: m.radiusMedium,
        bg: "raised",
        border: 1,
        borderColor: "lineStrong",
        occlude: true,
      },
      [
        ui.button("Add", {
          key: "canvas-add",
          icon: "plus",
          borderColor: "transparent",
          tooltip: "Add a node here (A)",
          onClick: (api) => api.act("canvas.add"),
        }),
        ui.box({ w: 1, h: 16, mx: 3, bg: "line" }),
        ui.iconButton("minus", {
          key: "canvas-zoom-out",
          tooltip: "Zoom out (⌘−)",
          onClick: (api) => api.act("canvas.zoomOut"),
        }),
        // A button of the app's look, around the percentage the canvas
        // keeps up to date itself.
        ui.row(
          {
            key: "canvas-zoom",
            id: "canvas-zoom",
            shrink: 0,
            w: 54,
            h: m.control,
            px: 10,
            items: "center",
            justify: "center",
            gap: 6,
            radius: m.radius,
            focusable: true,
            cursor: "pointer",
            font: "mono",
            size: m.textSmall,
            color: "fgDim",
            hover: { bg: "selected" },
            active: { opacity: 0.7 },
            tooltip: "Zoom to 100%, the selection, or everything",
            onClick: (api) => api.act("canvas.zoomMenu"),
          },
          [ui.slot("percent")],
        ),
        ui.iconButton("plus", {
          key: "canvas-zoom-in",
          tooltip: "Zoom in (⌘=)",
          onClick: (api) => api.act("canvas.zoomIn"),
        }),
        ui.iconButton("fit", {
          key: "canvas-fit",
          tooltip: "Fit everything (⌘0)",
          onClick: (api) => api.act("canvas.fit"),
        }),
      ],
    );
  },
});

// What an empty board offers: its templates, a first agent, a note.
// `ctx.data`: templates ([{ title, text, detail }]), offers (whether there
// is anything to start: else the app is asked which). Acts:
// canvas.template ({ index }), canvas.agent, canvas.note.
kou.view({
  id: "canvasEmpty",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const cards = [
      ...d.templates.map((template, index) => ({
        icon: "canvas",
        title: template.title,
        text: template.text,
        detail: template.detail,
        act: ["canvas.template", { index }],
      })),
      {
        icon: "thread",
        title: "An agent",
        text: d.offers
          ? "Pick what to start, in the middle of the board."
          : "Start one here; the app asks which.",
        act: ["canvas.agent"],
      },
      {
        icon: "note",
        title: "A note",
        text: "Text on the board, for the plan or what to remember.",
        act: ["canvas.note"],
        quiet: true,
      },
    ];
    const card = (card, index) => {
      const group = "card-" + index;
      return ui.col(
        {
          key: "canvas-card-" + index,
          id: "canvas-card-" + index,
          group,
          w: 220,
          minH: 128,
          p: 14,
          gap: 8,
          radius: m.radiusLarge,
          border: 1,
          borderColor: "lineStrong",
          dashed: card.quiet,
          bg: card.quiet ? undefined : "raised",
          cursor: "pointer",
          hover: { bg: "hover" },
          active: { opacity: 0.8 },
          ring: { borderColor: "accent" },
          stopPress: true,
          onClick: (api) => api.act(card.act[0], card.act[1]),
        },
        [
          ui.row({ items: "center", gap: 10 }, [
            ui.row(
              {
                shrink: 0,
                w: 28,
                h: 28,
                radius: m.radius,
                bg: "selected",
                items: "center",
                justify: "center",
              },
              [ui.icon(card.icon, { color: "fgMuted", groupHover: { group, color: "fg" } })],
            ),
            ui.text(card.title, { minW: 0, size: m.text, weight: "medium", color: "fg" }),
          ]),
          ui.text(card.text, { grow: 1, size: 12, lineHeight: 18, color: "fgDim" }),
          card.detail != null &&
            ui.text(card.detail, { size: m.textSmall, lineHeight: 16, color: "fgMuted" }),
        ],
      );
    };
    return ui.col(
      { position: "absolute", w: "full", h: "full", items: "center", justify: "center", pb: 64 },
      [
        ui.col({ p: 36, items: "center", gap: 20 }, [
          ui.col({ maxW: 520, items: "center", gap: 8 }, [
            ui.text("An empty canvas", { size: 15, color: "fg" }),
            ui.text(
              "Agents, terminals and notes, wired output to input: what one puts out goes into the next. Start from a template, add a node (A, or double-click the board), or right-click for more.",
              { align: "center", size: m.text, lineHeight: 20, color: "fgMuted" },
            ),
          ]),
          ui.row({ wrap: true, justify: "center", gap: 10, maxW: 960 }, cards.map(card)),
        ]),
      ],
    );
  },
});

// The open menu, where the canvas puts it. `ctx.data`: x, y, width, height,
// title, highlight (the row the keys are on), items ([{ label, detail,
// hint, mark, danger, enabled, separator }]: `mark` is a harness's
// `{ code }`, an icon's `{ icon, note }`, or null). Acts: canvas.run and
// canvas.hover ({ index }), canvas.close.
kou.view({
  id: "canvasMenu",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const row = (item, index) => {
      const lit = d.highlight === index;
      let mark;
      if (item.mark && item.mark.code) {
        mark = ui.monogram(item.mark.code, { size: 16 });
      } else if (item.mark && item.mark.icon) {
        // A node's own mark keeps its colour; the row's icon lights with it.
        const ofNode = "note" in item.mark;
        const color = ofNode
          ? item.mark.note
            ? "blocked"
            : "fgMuted"
          : item.danger
            ? "error"
            : lit && item.enabled
              ? "fg"
              : "fgMuted";
        mark = ui.icon(item.mark.icon, { color });
      } else {
        mark = ui.box({ shrink: 0, w: m.icon });
      }
      return ui.box({}, [
        item.separator &&
          ui.row({ h: 9, px: PAD + 8, items: "center" }, [ui.box({ w: "full", h: 1, bg: "line" })]),
        ui.row(
          {
            id: "menu-row-" + index,
            key: "menu-row-" + index,
            h: m.row,
            mx: PAD,
            px: 8,
            items: "center",
            gap: 8,
            radius: m.radius,
            cursor: item.enabled ? "pointer" : undefined,
            onHover: (api, event) => event.hovered && api.act("canvas.hover", { index }),
            onPress: (api) => item.enabled && api.act("canvas.run", { index }),
          },
          [
            mark,
            ui.text(item.label, {
              shrink: 0,
              size: m.text,
              color: !item.enabled ? "fgMuted" : item.danger ? "error" : "fg",
            }),
            ui.box(
              { grow: 1, minW: 0, truncate: true, size: m.textSmall, color: "fgMuted" },
              item.detail != null ? [item.detail] : [],
            ),
            item.hint != null && keyHint(item.hint, m),
          ],
        ),
      ]);
    };
    return ui.box(
      {
        key: "canvas-menu",
        id: "canvas-menu",
        position: "absolute",
        left: d.x,
        top: d.y,
        w: d.width,
        h: d.height,
        radius: m.radiusLarge,
        overflow: "hidden",
        occlude: true,
        bg: "raised",
        border: 1,
        borderColor: "lineStrong",
        onOutside: (api) => api.act("canvas.close"),
      },
      [
        ui.slot("highlight"),
        ui.col({ position: "absolute", left: 0, top: 0, w: "full", py: PAD }, [
          d.title != null &&
            ui.row({ h: 26, px: PAD + 8, items: "center", size: 12, color: "fgMuted" }, [
              ui.text(d.title, { truncate: true }),
            ]),
          d.items.map(row),
        ]),
      ],
    );
  },
});

// The bar that says what was just deleted, and takes it back. `ctx.data`:
// text, undo (whether it can be taken back). Act: canvas.undo.
kou.view({
  id: "canvasToast",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    return ui.row(
      {
        items: "center",
        gap: 10,
        h: 40,
        pl: 14,
        pr: PAD + 3,
        radius: m.radiusMedium,
        bg: "raised",
        border: 1,
        borderColor: "lineStrong",
        occlude: true,
        size: m.text,
        color: "fgDim",
      },
      [
        ui.icon(d.undo ? "trash" : "info", { color: "fgMuted" }),
        d.text,
        d.undo && [
          ui.button("Undo", {
            key: "canvas-undo",
            icon: "undo",
            stopPress: true,
            onClick: (api) => api.act("canvas.undo"),
          }),
          keyHint("⌘Z", m),
        ],
      ],
    );
  },
});

// A node's mark: its harness's tile, or its kind's icon (a note's in the
// note's colour).
function nodeMark(mark) {
  if (!mark) return null;
  if (mark.code) return ui.monogram(mark.code, { size: 16 });
  return ui.icon(mark.icon, { color: mark.note ? "blocked" : "fgMuted" });
}

// The inspector: a panel over the right of the board with what is selected.
// `ctx.data`: kind (`node`, `nodes`, `edge`), title, mark, editing (which of
// its fields is being typed in: `title`, `note`, `template`; that one is the
// slot `field`), and `node`, `nodes` or `edge` with what it shows of them.
// The canvas slides it in and out. Acts: canvas.inspectorClose,
// canvas.edit ({ what, node | edge }), canvas.pickNode, canvas.pickEdge,
// canvas.wireFrom, canvas.orchestrator ({ node, on }), canvas.enter,
// canvas.open, canvas.focusNode, canvas.duplicate, canvas.delete,
// canvas.zoomSelection, canvas.deleteSelection, canvas.edgeEnds ({ edge,
// end }), canvas.edgeMode, canvas.edgeDelivery, canvas.edgeHeader ({ edge,
// index }), canvas.deleteEdge.
kou.view({
  id: "canvasInspector",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const act = (name, args) => (api) => api.act("canvas." + name, args);
    // A part of the panel, under a line, with its icon and heading.
    const section = (icon, title, children) =>
      ui.col({ gap: 10, py: 14, borderB: 1, borderColor: "line" }, [
        title &&
          ui.row({ items: "center", gap: 6 }, [
            icon && ui.icon(icon, { color: "fgMuted" }),
            ui.text(title, { size: 12, color: "fgMuted" }),
          ]),
        children,
      ]);
    const note = (text) => ui.text(text, { size: m.textSmall, lineHeight: 16, color: "fgMuted" });
    // A value that becomes a field when pressed (or Enter on it).
    const editable = (key, what, value, placeholder, args) => {
      if (d.editing === what) return ui.slot("field");
      // A name or a note is prose; a template is closer to code.
      const machine = what === "template";
      // Its lines as the app counts them: a last newline starts none.
      let lines = [placeholder];
      if (value) {
        lines = value.split("\n").map((line) => line.replace(/\r$/, ""));
        if (value.endsWith("\n")) lines.pop();
      }
      return ui.box(
        {
          key,
          id: key,
          minH: 32,
          px: 10,
          py: 7,
          radius: m.radius,
          border: 1,
          borderColor: "lineStrong",
          bg: "pane",
          font: machine ? "mono" : undefined,
          size: machine ? 12 : m.text,
          lineHeight: 17,
          color: value ? "fg" : "fgMuted",
          cursor: "text",
          hover: { borderColor: "fgFaint" },
          ring: { borderColor: "accent" },
          onClick: act("edit", { what, ...args }),
        },
        lines.map((line) => ui.text(line, { truncate: true })),
      );
    };
    // A row to pick, as the rail's rows are: the mark of what it leads to.
    const choice = (key, mark, label, onClick) =>
      ui.row(
        {
          key,
          id: key,
          h: m.row,
          mx: -8,
          px: 8,
          items: "center",
          gap: 8,
          radius: m.radius,
          size: 12,
          color: "fgDim",
          cursor: "pointer",
          focusable: true,
          hover: { bg: "hover" },
          onClick,
        },
        [nodeMark(mark), ui.text(label, { minW: 0, truncate: true })],
      );
    const button = (key, icon, label, onClick, props) =>
      ui.button(label, { key, id: key, icon, size: 12, onClick, ...props });
    const danger = { color: "error" };
    // A choice of a few, as one strip of buttons with the chosen one filled.
    const segmented = (key, options, pick) =>
      ui.row(
        {
          gap: 2,
          p: 2,
          radius: m.radius,
          border: 1,
          borderColor: "lineStrong",
          bg: "pane",
        },
        options.map((option, index) =>
          ui.row(
            {
              key: key + "-" + index,
              id: key + "-" + index,
              grow: 1,
              h: 24,
              items: "center",
              justify: "center",
              radius: m.radiusSmall,
              size: 12,
              color: option.on ? "fg" : "fgMuted",
              bg: option.on ? "selected" : undefined,
              weight: option.on ? "medium" : undefined,
              hover: option.on ? undefined : { bg: "hover" },
              cursor: "pointer",
              focusable: true,
              onClick: pick(index),
            },
            [option.label],
          ),
        ),
      );

    let body = [];
    let mark;
    if (d.kind === "node") {
      const n = d.node;
      mark = nodeMark(d.mark);
      const STATE = { working: "working", done: "done", blocked: "blocked", failed: "error" };
      body.push(
        section(null, null, [
          editable("inspector-title", "title", n.title, "Title", { node: n.id }),
          ui.row({ justify: "between", size: m.textSmall, color: "fgMuted" }, [
            n.kind,
            ui.text("#" + n.id, { font: "mono" }),
          ]),
          !n.note &&
            ui.row({ items: "center", gap: 8, size: m.textSmall }, [
              ui.mark(n.state),
              ui.text(n.word, { color: STATE[n.state] || "fgMuted" }),
              n.since != null && ui.text("for " + n.since, { color: "fgMuted" }),
            ]),
          n.place &&
            ui.row({ items: "center", gap: 8, size: m.textSmall }, [
              ui.icon("folder", { color: "fgMuted" }),
              ui.text("Where", { shrink: 0, color: "fgMuted" }),
              ui.text(n.place, {
                grow: 1,
                minW: 0,
                truncate: true,
                align: "right",
                font: "mono",
                color: "fg",
              }),
            ]),
        ]),
      );
      if (n.note) {
        body.push(
          section("note", "Note", [editable("inspector-note", "note", n.detail, "Text", { node: n.id })]),
        );
      }
      if (n.wires) {
        body.push(
          section("wire", "Wires", [
            n.wires.length === 0 &&
              note("Nothing wired. Drag from a port on its side, or wire it here."),
            // The rows sit together, as the rail's do.
            ui.col(
              {},
              n.wires.map((wire) =>
                choice("inspector-wire-" + wire.edge, wire.mark, wire.label, act("pickEdge", { edge: wire.edge })),
              ),
            ),
            n.outputs &&
              ui.row({}, [
                button("inspector-wire-to", "wire", "Wire its output to…", act("wireFrom", { node: n.id })),
              ]),
          ]),
        );
      }
      // Who else may wire this board: the agent of a node the user made its
      // orchestrator.
      if (n.agent) {
        body.push(
          section("wire", "Orchestrator", [
            segmented(
              "inspector-orchestrator",
              [
                { label: "Off", on: !n.orchestrator },
                { label: "On", on: n.orchestrator },
              ],
              (index) => act("orchestrator", { node: n.id, on: index === 1 }),
            ),
            note(
              n.orchestrator
                ? "Its agent may wire the nodes of this tab together and take wires away, as you do. What it wires says that it did."
                : "Only you wire this tab. Turned on, this node's agent may too.",
            ),
          ]),
        );
      }
      body.push(
        section(null, null, [
          ui.row({ wrap: true, gap: 6 }, [
            n.body &&
              button("inspector-enter", "chevron-right", "Enter", act("enter", { node: n.id }), {
                tooltip: "Its body takes the keys (Enter)",
              }),
            n.pane && button("inspector-open", "pane", "Open as a pane", act("open", { node: n.id })),
            button("inspector-focus", "target", "Focus", act("focusNode", { node: n.id }), {
              tooltip: "At 100%, in the middle (F)",
            }),
            button("inspector-duplicate", "duplicate", "Duplicate", act("duplicate", { node: n.id })),
            button("inspector-delete", "trash", "Delete", act("delete", { node: n.id }), danger),
          ]),
        ]),
      );
    } else if (d.kind === "nodes") {
      mark = ui.icon("canvas", { color: "fgMuted" });
      body = [
        section(null, null, [
          note("Pick one to inspect it alone."),
          ui.col(
            {},
            d.nodes.map((node) =>
              choice("inspector-pick-" + node.id, node.mark, node.title, act("pickNode", { node: node.id })),
            ),
          ),
        ]),
        section(null, null, [
          ui.row({ gap: 6 }, [
            button("inspector-fit-selection", "target", "Zoom to them", act("zoomSelection")),
            button("inspector-delete-all", "trash", "Delete", act("deleteSelection"), danger),
          ]),
        ]),
      ];
    } else {
      const e = d.edge;
      mark = ui.icon("wire", { color: "accent" });
      const end = (side) =>
        ui.row({ items: "center", gap: 6 }, [
          nodeMark(side.mark),
          ui.text(side.title, { minW: 0, truncate: true, color: "fg", weight: "medium" }),
          ui.text(side.port, { font: "mono", size: m.textSmall, color: "fgMuted" }),
        ]);
      body = [
        section(null, null, [
          ui.col({ gap: 6, size: m.text }, [
            end(e.from),
            ui.text("↓", { pl: 4, color: "fgMuted" }),
            end(e.to),
          ]),
          note(e.origin),
          ui.row({ wrap: true, gap: 6 }, [
            button("inspector-move-start", "wire", "Move its start…", act("edgeEnds", { edge: e.id, end: "from" })),
            button("inspector-move-end", "wire", "Move its end…", act("edgeEnds", { edge: e.id, end: "to" })),
          ]),
        ]),
        section("sliders", "Mode", [
          segmented("inspector-mode", e.modes, (index) => act("edgeMode", { edge: e.id, index })),
          note(e.mode),
        ]),
        section("queue", "When the target is busy", [
          segmented("inspector-delivery", e.deliveries, (index) =>
            act("edgeDelivery", { edge: e.id, index }),
          ),
          note(e.delivery),
        ]),
        section("info", "A line naming where it comes from", [
          segmented("inspector-header", e.headers, (index) => act("edgeHeader", { edge: e.id, index })),
        ]),
        section("pencil", "Template", [
          editable("inspector-template", "template", e.template, "{{text}}", { edge: e.id }),
          note("{{text}} {{from.title}} {{from.id}} {{from.port}} {{edge.id}}"),
          ui.text("What an output makes", { size: 12, color: "fgMuted" }),
          ui.box(
            {
              p: 8,
              radius: m.radius,
              border: 1,
              borderColor: "line",
              bg: "pane",
              font: "mono",
              size: 11.5,
              lineHeight: 17,
              color: "fgDim",
            },
            e.preview.map((line) => ui.text(line)),
          ),
        ]),
        section("thread", "Messages", [
          note(e.messages),
          ui.row({}, [
            button("inspector-delete-edge", "trash", "Delete the edge", act("deleteEdge", { edge: e.id }), danger),
          ]),
        ]),
      ];
    }
    return ui.col(
      {
        w: "full",
        h: "full",
        radius: m.radiusLarge,
        overflow: "hidden",
        bg: "raised",
        border: 1,
        borderColor: "lineStrong",
        occlude: true,
      },
      [
        ui.row(
          {
            shrink: 0,
            h: m.bar + 6,
            pl: 14,
            pr: 10,
            items: "center",
            gap: 8,
            borderB: 1,
            borderColor: "line",
          },
          [
            mark,
            ui.text(d.title, {
              grow: 1,
              minW: 0,
              truncate: true,
              size: m.text,
              weight: "medium",
              color: "fg",
            }),
            ui.iconButton("close", {
              key: "inspector-close",
              id: "inspector-close",
              tooltip: "Close (I or Esc)",
              onClick: act("inspectorClose"),
            }),
          ],
        ),
        ui.box(
          { key: "inspector-body", grow: 1, minH: 0, overflow: "scroll", px: 14, pb: 16 },
          body,
        ),
      ],
    );
  },
});

// A node's header, drawn as it is at 100% and scaled with the board by the
// canvas (so a pan or a zoom draws it again without asking for it again).
// Its harness's mark, its name and its state's dot stay the canvas's own
// (`ui.slot("glyph")`, `ui.slot("title")`, `ui.slot("mark")`), and so does
// taking hold of it to move the node. `ctx.data`: node, kind, note, editing
// (its name is being typed in), state, word, elapsed (how long it has
// worked, while it does), reachable (Tab reaches its menu's button: the
// selected node's). Act: canvas.nodeMenu ({ node }).
const NOTE_SURFACE = "mix(pane, blocked, 0.05)";
kou.view({
  id: "canvasHead",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const STATE = { working: "working", done: "done", blocked: "blocked", failed: "error" };
    return ui.row(
      {
        h: 33,
        pl: 10,
        pr: 5,
        items: "center",
        gap: 8,
        bg: d.note ? NOTE_SURFACE : "hover",
        // A hairline at any zoom, as the card's own lines are.
        borderB: "1px",
        borderColor: d.note ? "mix(line, blocked, 0.18)" : "line",
        overflow: "hidden",
      },
      [
        ui.slot("glyph"),
        ui.slot("title"),
        !d.note &&
          !d.editing &&
          ui.text(d.kind, { shrink: 1, minW: 0, truncate: true, size: m.textSmall, color: "fgMuted" }),
        ui.box({ grow: 1 }),
        !d.note &&
          ui.row({ shrink: 0, items: "center", gap: 5, size: m.textSmall }, [
            d.elapsed != null && ui.text(d.elapsed, { color: "fgMuted" }),
            d.state !== "idle" && ui.text(d.word, { color: STATE[d.state] }),
            ui.slot("mark"),
          ]),
        // Its menu.
        ui.row(
          {
            key: "more",
            shrink: 0,
            w: 24,
            h: 24,
            radius: m.radius,
            items: "center",
            justify: "center",
            border: "1px",
            borderColor: "transparent",
            cursor: "pointer",
            hover: { bg: "selected" },
            active: { opacity: 0.6 },
            ring: d.reachable ? { borderColor: "accent" } : undefined,
            stopPress: true,
            onClick: (api) => api.act("canvas.nodeMenu", { node: d.node }),
          },
          [ui.icon("more", { color: "fgMuted" })],
        ),
      ],
    );
  },
});

// A node's footer: where it works. `ctx.data`: place, note.
kou.view({
  id: "canvasFoot",
  render(ctx) {
    const d = ctx.data;
    return ui.row(
      {
        h: 22,
        px: 10,
        items: "center",
        gap: 6,
        borderT: "1px",
        borderColor: "line",
        bg: d.note ? NOTE_SURFACE : "pane",
        font: "mono",
        size: 10.5,
        color: "fgMuted",
      },
      [ui.icon("folder", { size: 12, color: "fgMuted" }), ui.text(d.place, { minW: 0, truncate: true })],
    );
  },
});

// An edge's label: what it says (who wired it when it was not you, whether
// it is off or asks first, how many went along it), in the box the canvas
// gives it at the middle of its edge, at the board's zoom (it is made at
// 100%); a press on it is the canvas's. `ctx.data`: edge, text, waiting
// (messages on it wait for your approval), selected.
kou.view({
  id: "canvasLabel",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const tint = d.selected ? "accent" : d.waiting ? "blocked" : null;
    return ui.row(
      {
        id: "edge-label-" + d.edge,
        w: "full",
        h: "full",
        items: "center",
        justify: "center",
        radius: m.radiusSmall,
        bg: "raised",
        border: "1px",
        borderColor: tint || "lineStrong",
        font: "mono",
        size: 10,
        weight: "medium",
        color: tint || "fgMuted",
        cursor: "pointer",
      },
      [d.text],
    );
  },
});

// The tab over a node with what waits for it: messages along its edges,
// and how many of them ask for your approval. The canvas hangs it over the
// node, at the node's zoom (over a placeholder, at 100%). `ctx.data`: node,
// text, approve. Act: canvas.pendingOpened ({ node }).
kou.view({
  id: "canvasPending",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const ink = d.approve ? "blocked" : "fgDim";
    return ui.row(
      {
        key: "pending",
        id: "pending-" + d.node,
        h: 22,
        px: 8,
        items: "center",
        gap: 5,
        radius: m.radius,
        border: "1px",
        borderColor: d.approve ? "mix(lineStrong, blocked, 0.55)" : "lineStrong",
        bg: "raised",
        size: m.textSmall,
        weight: "medium",
        color: ink,
        cursor: "pointer",
        nowrap: true,
        hover: { bg: "selected" },
        ring: { borderColor: "accent" },
        stopPress: true,
        onClick: (api) => api.act("canvas.pendingOpened", { node: d.node }),
      },
      [ui.icon("queue", { size: 12, color: ink }), d.text],
    );
  },
});

// What a node with no live body says, under its header: what it was given
// to show, a line at a time, or a note's text; when it has none, what is to
// be done. Made at 100% and drawn at the board's zoom; holding it moves the
// node, which is the canvas's. `ctx.data`: note, empty, lines.
kou.view({
  id: "canvasBody",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const room = d.note ? { px: 14, py: 10 } : { p: 10 };
    if (d.empty) {
      return ui.box(room, [
        ui.text(d.note ? "Double-click to write." : "Nothing yet.", { size: 12, color: "fgMuted" }),
      ]);
    }
    if (d.note) {
      return ui.box(
        { ...room, size: m.text, lineHeight: 20, color: "fg" },
        d.lines.map((line) => ui.text(line, { minH: 20 })),
      );
    }
    return ui.box(
      { ...room, font: "mono", size: 11.5, lineHeight: 17, color: "fgDim" },
      d.lines.map((line) => ui.text(line, { truncate: true })),
    );
  },
});
