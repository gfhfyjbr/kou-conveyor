// Transcript prose, image decoding and layout measurements are native slots.
// These views own the surrounding presentation and dispatch semantic actions.
const action = (id, glyph, name, index) =>
  ui.row(
    {
      id,
      key: id,
      w: 20,
      h: 20,
      radius: 6,
      shrink: 0,
      items: "center",
      justify: "center",
      opacity: 0.45,
      cursor: "pointer",
      border: "1px",
      borderColor: "transparent",
      hover: { bg: "hover", opacity: 1 },
      ring: { borderColor: "accent", opacity: 1 },
      onClick: (api, event) =>
        api.act(name, { index, keyboard: event.keyboard }),
    },
    [ui.icon(glyph, { size: 12, color: "fgMuted" })],
  );
kou.view({
  id: "studio-user",
  render(ctx) {
    const d = ctx.data;
    return ui.col({ w: "full", gap: 6 }, [
      d.pictures && ui.slot("pictures"),
      ui.row({ w: "full", justify: "end", items: "start", gap: 4 }, [
        ui.row({ shrink: 0, pt: 4, gap: 2 }, [
          action(
            "copy-prompt-" + d.index,
            d.copied ? "check" : "copy",
            "copy",
            d.index,
          ),
          d.again &&
            action(
              d.again + "-prompt-" + d.index,
              d.again === "edit" ? "pencil" : "undo",
              d.again,
              d.index,
            ),
        ]),
        ui.col(
          {
            id: "bubble-" + d.index,
            position: "relative",
            minW: 0,
            maxW: "85%",
            px: 12,
            py: 6,
            radius: 12,
            bg: "selected",
            size: 13,
            lineHeight: 22,
            color: "fg",
            gap: 2,
          },
          [
            ui.slot("measure"),
            d.text && ui.slot("text"),
            d.files.map((path) =>
              ui.row({ items: "center", gap: 6, size: 12.46, color: "fgDim" }, [
                ui.icon("file", { size: 12, color: "fgMuted" }),
                ui.text(path, { minW: 0, truncate: true }),
              ]),
            ),
          ],
        ),
      ]),
    ]);
  },
});
kou.view({
  id: "studio-answer",
  render(ctx) {
    const d = ctx.data;
    return ui.box(
      { id: "reply-" + d.index, position: "relative", w: "full", px: 3 },
      [
        ui.slot("prose"),
        !d.writing &&
          ui.box({ position: "absolute", top: 0, right: -24 }, [
            action(
              "copy-answer-" + d.index,
              d.copied ? "check" : "copy",
              "copy",
              d.index,
            ),
          ]),
      ],
    );
  },
});
kou.view({
  id: "studio-thought",
  render(ctx) {
    const d = ctx.data;
    return ui.col({ w: "full" }, [
      ui.row(
        {
          id: "thought-" + d.index,
          key: "fold",
          h: 24,
          w: "full",
          items: "center",
          gap: 8,
          radius: ctx.theme.metric.radius,
          border: "1px",
          borderColor: "transparent",
          cursor: "pointer",
          size: 13,
          color: "fgMuted",
          hover: { color: "fgDim" },
          ring: { borderColor: "accent" },
          onClick: (api, event) =>
            api.act("toggle", {
              index: d.index,
              open: !d.open,
              keyboard: event.keyboard,
            }),
        },
        [
          ui.text(d.writing ? "Thinking" : "Thought", { shrink: 0 }),
          ui.text(d.gist, {
            grow: 1,
            minW: 0,
            truncate: true,
            italic: true,
            color: "fgMuted",
          }),
          ui.icon(d.open ? "chevron-down" : "chevron-right", {
            size: 12,
            color: "fgMuted",
          }),
        ],
      ),
      d.open &&
        ui.box({ my: 4, pl: 12, borderL: "1px", borderColor: "lineStrong" }, [
          ui.slot("prose"),
        ]),
    ]);
  },
});
kou.view({
  id: "studio-tool-head",
  render(ctx) {
    const d = ctx.data;
    const glyph =
      d.status === "working" || d.status === "blocked"
        ? ui.box({ w: 7, h: 7, radius: "full", bg: d.status })
        : d.status === "done" || d.status === "error"
          ? ui.icon(d.status === "done" ? "check" : "close", {
              size: 12,
              color: d.status === "error" ? "error" : "fgFaint",
            })
          : ui.box({
              w: 7,
              h: 7,
              radius: "full",
              border: "1px",
              borderColor: "fgFaint",
            });
    return ui.row(
      {
        id: "tool-" + d.index,
        key: "call",
        h: 24,
        mx: -6,
        px: 6,
        items: "center",
        gap: 8,
        size: 13,
        radius: ctx.theme.metric.radius,
        border: "1px",
        borderColor: "transparent",
        cursor: d.expandable ? "pointer" : undefined,
        hover: d.expandable ? { bg: "hover" } : undefined,
        ring: d.expandable ? { borderColor: "accent" } : undefined,
        onClick: d.expandable
          ? (api, event) =>
              api.act("toggle", {
                index: d.index,
                open: !d.open,
                keyboard: event.keyboard,
              })
          : undefined,
      },
      [
        ui.row({ w: 14, shrink: 0, items: "center", justify: "center" }, [
          glyph,
        ]),
        ui.text(d.verb, { shrink: 0, color: "fgDim" }),
        d.input &&
          ui.text(d.input, {
            minW: 0,
            shrink: 1,
            truncate: true,
            font: "mono",
            size: 12.46,
            color: "fgMuted",
          }),
        d.location &&
          ui.text(d.location, {
            minW: 0,
            shrink: 1000,
            truncate: "start",
            font: "mono",
            size: 12.46,
            color: "fgMuted",
          }),
        ui.box({ grow: 1 }),
        ui.row(
          { shrink: 0, items: "center", gap: 8, font: "mono", size: 12.46, color: "fgMuted" },
          [
            d.added > 0 && ui.text("+" + d.added, { color: "ok" }),
            d.removed > 0 && ui.text("-" + d.removed, { color: "error" }),
            d.took && ui.text(d.took),
            d.word && ui.text(d.word.text, { font: "ui", color: d.word.color }),
            d.expandable &&
              ui.icon(d.open ? "chevron-down" : "chevron-right", {
                size: 12,
                color: "fgMuted",
              }),
          ],
        ),
      ],
    );
  },
});

kou.view({
  id: "studio-dock",
  render(ctx) {
    const d = ctx.data;
    const slot = (name) => (d.slots.includes(name) ? ui.slot(name) : null);
    return ui.row(
      {
        w: "full",
        shrink: 0,
        pr: d.inset,
        justify: "center",
        bg: d.standalone ? undefined : "pane",
      },
      [
        ui.col(
          {
            position: "relative",
            w: "full",
            maxW: d.width,
            shrink: 0,
            px: d.side,
            pb: d.standalone ? 0 : d.under,
          },
          [
            slot("measure"),
            slot("activity"),
            slot("queue"),
            slot("frame"),
            slot("usage"),
            slot("help"),
            d.slots.includes("modelMenu") &&
              ui.box({ position: "absolute", top: 0, right: d.side }, [
                slot("modelMenu"),
              ]),
          ],
        ),
      ],
    );
  },
});
kou.view({
  id: "studio-activity",
  render(ctx) {
    const d = ctx.data;
    return ui.row({ id: "activity", h: 22, items: "center", size: 11.48 }, [
      ui.box({ shrink: 0, ml: 3, w: 11.4, h: 11.4 }, [ui.slot("meter")]),
      ui.text(d.words, { shrink: 0, ml: 6.2, color: "fgDim" }),
      d.titles &&
        ui.text(d.titles, {
          minW: 0,
          grow: 1,
          ml: 8,
          truncate: true,
          color: "fgMuted",
        }),
      !d.titles && ui.box({ grow: 1 }),
      d.stop &&
        ui.text("Stop", {
          id: "waiting-stop",
          key: "stop",
          shrink: 0,
          ml: 10.6,
          radius: ctx.theme.metric.radius,
          border: "1px",
          borderColor: "transparent",
          cursor: "pointer",
          weight: "medium",
          color: "fg",
          hover: { color: "fgDim" },
          ring: { borderColor: "accent" },
          onClick: (api) => api.act("stop"),
        }),
    ]);
  },
});

kou.view({
  id: "studio-plan",
  render(ctx) {
    const steps = ctx.data.steps;
    return ui.col({ w: "full" }, [
      ui.text(
        "Plan · " +
          steps.filter((s) => s.status === "done").length +
          " of " +
          steps.length +
          " done",
        { h: 24, size: 13, color: "fgMuted" },
      ),
      steps.map((step) =>
        ui.row({ items: "start", gap: 8 }, [
          ui.row(
            { w: 14, h: 22, shrink: 0, items: "center", justify: "center" },
            [
              step.status === "running"
                ? ui.slot("mark")
                : step.status === "done"
                  ? ui.icon("check", { size: 12, color: "ok" })
                  : ui.box({
                      w: 8,
                      h: 8,
                      radius: "full",
                      border: "1px",
                      borderColor: "fgFaint",
                    }),
            ],
          ),
          ui.text(step.text, {
            minW: 0,
            size: 13,
            lineHeight: 22,
            strike: step.status === "done",
            color:
              step.status === "done"
                ? "fgMuted"
                : step.status === "running"
                  ? "fg"
                  : "fgDim",
          }),
        ]),
      ),
    ]);
  },
});
kou.view({
  id: "studio-notice",
  render(ctx) {
    const d = ctx.data;
    if (d.level === "info")
      return ui.row(
        {
          w: "full",
          minH: 22,
          items: "center",
          gap: 10,
          size: 12.46,
          color: "fgMuted",
        },
        [
          ui.text(d.text, { minW: 0 }),
          ui.box({ grow: 1, minW: 16, h: 1, bg: "line" }),
        ],
      );
    const color = d.level === "warning" ? "blocked" : "error";
    return ui.col({ w: "full", gap: 4 }, [
      ui.text(
        d.level === "warning" ? "Warning" : d.turn ? "Turn failed" : "Error",
        { size: 12.46, color },
      ),
      ui.text(d.text, {
        px: 12,
        py: 8,
        radius: 8,
        bg: ui.alpha(color, 0.06),
        border: "1px",
        borderColor: ui.alpha(color, 0.22),
        size: 13,
        lineHeight: 22,
        color,
      }),
    ]);
  },
});
kou.view({
  id: "studio-worked",
  render(ctx) {
    const d = ctx.data;
    return ui.col({ w: "full", items: "start" }, [
      ui.row(
        {
          id: "worked-" + d.start,
          key: "worked",
          h: 22,
          ml: 4.7,
          items: "center",
          gap: 5,
          radius: ctx.theme.metric.radius,
          border: "1px",
          borderColor: "transparent",
          cursor: "pointer",
          size: 13,
          color: "fgMuted",
          hover: { color: "fgDim" },
          ring: { borderColor: "accent" },
          onClick: (api, event) =>
            api.act("worked", { start: d.start, keyboard: event.keyboard }),
        },
        [
          ui.text(d.text),
          ui.icon(d.open ? "chevron-down" : "chevron-right", {
            size: 12,
            color: "fgMuted",
          }),
        ],
      ),
      ui.box({ mt: 4.925, w: "full", h: 1, bg: "line" }),
    ]);
  },
});
kou.view({
  id: "studio-status",
  render: (ctx) =>
    ui.text(ctx.data.text, { minH: 22, size: 13, color: "fgMuted" }),
});
kou.view({
  id: "studio-usage",
  render: (ctx) =>
    ui.text(ctx.data.text, {
      id: "usage",
      h: 22,
      mt: 2,
      px: 3,
      size: 11.48,
      color: "fgMuted",
    }),
});
kou.view({
  id: "studio-ready",
  render: (ctx) =>
    ui.col({ w: "full", pt: 72, items: "center", gap: 6 }, [
      ui.text("What should " + ctx.data.agent + " work on?", {
        size: 20,
        lineHeight: 28,
        weight: "medium",
        color: "fg",
      }),
      ui.text(ctx.data.path, { size: 13, color: "fgMuted" }),
    ]),
});
kou.view({
  id: "studio-cleared",
  render(ctx) {
    const n = ctx.data.hidden;
    return ui.row(
      { minH: 22, items: "center", gap: 10, size: 13, color: "fgMuted" },
      [
        ui.text(
          n === 1
            ? "1 earlier entry is out of sight"
            : n + " earlier entries are out of sight",
        ),
        ui.text("Show them", {
          id: "unclear",
          key: "unclear",
          radius: ctx.theme.metric.radius,
          border: "1px",
          borderColor: "transparent",
          color: "fgDim",
          cursor: "pointer",
          hover: { color: "fg" },
          ring: { borderColor: "accent" },
          onClick: (api) => api.act("unclear"),
        }),
        ui.box({ grow: 1, h: 1, bg: "line" }),
      ],
    );
  },
});
