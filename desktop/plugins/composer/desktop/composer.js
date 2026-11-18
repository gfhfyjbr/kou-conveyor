// The prompt field: the frame a prompt is typed in, in a chat and on the
// empty page, with its controls, and what stands around it in a chat (the
// line over it while the agent works, the queue, the figures under it). How
// it all looks and how its parts are arranged is here, and a change to this
// file shows in the open window as it is saved.
//
// What stays the app's own is placed with `ui.slot(name)`, each once:
//   editor    what is typed, with its chips, pictures and menus
//   foot      what the app puts at the foot's start (who gets the prompt,
//             on the empty page)
//   modelMenu  the model picker, while it is open (over the model's control)
//   modeMenu  the modes' list, while it is open (over the mode's control)
//   effortMenu  the levels' list, while it is open (over the effort's)
// `ctx.data.slots` names the ones the field has now. The rest of
// `ctx.data` says how the field stands: `focused`, `busy` (its agent is at
// work), `closed` (its agent is gone), `narrow`, `standalone` (no
// conversation over it), and `controls`, how its controls stand:
//   buttons   "send"; while the agent works "stop", or "force" once
//             something is typed (force it in, or queue it)
//   ready     what is typed can be sent
//   attach    the paperclip is there
//   model     { name, open, dot }: the model's control, `dot` its
//             provider's colour; null when there is none
//   mode      { name, open, effort }: the mode's control, `effort` being
//             { level, bars } where the modes are efforts; null when none
//   effort    { name, open, effort }: the control of the levels of effort
//             an agent offers apart from its modes, drawn as kou's efforts
//             are; null (or not said, by an older app) when there is none
// The controls of the foot are one height but the button at its end, which
// is the one that acts and is two pixels taller.
// An app that does not say how its controls stand draws them itself, as
// the slots `attach`, `model`, `mode` and `controls`.
//
// A control asks the field with `api.act`: `field.send` (queued while the
// agent works), `field.force`, `field.stop`, `field.attach`, `field.focus`,
// `field.model`, `field.mode` and `field.effort` (open or close their lists).

// The look of the web cockpit's `.label`: a word in capitals of the
// monospace.
const label = (text, color) =>
  ui.text(String(text).toUpperCase(), {
    shrink: 0,
    font: "mono",
    weight: "bold",
    size: 10.5,
    color,
  });

// A key, as the app shows one to press.
const key = (keys, m) =>
  ui.text(keys, {
    shrink: 0,
    px: 4,
    radius: m.radiusSmall,
    // (Borders are in the screen's pixels, as the chat's own are: a
    // hairline stays one at any zoom of a canvas.)
    border: "1px",
    borderColor: "lineStrong",
    font: "mono",
    size: 10.5,
    color: "fgMuted",
  });

// The web cockpit's `.act`: a word in capitals of the monospace, framed
// only under the pointer or the keys.
const act = (id, text, m, onClick, glyph) =>
  ui.row(
    {
      id,
      key: id,
      shrink: 0,
      h: 22,
      px: 7,
      radius: m.radius,
      border: "1px",
      borderColor: "transparent",
      items: "center",
      gap: glyph ? 5 : undefined,
      cursor: "pointer",
      font: "mono",
      weight: "semibold",
      size: 10,
      color: "fgMuted",
      hover: glyph
        ? { borderColor: "lineStrong", color: "fg" }
        : { borderColor: "lineStrong", bg: "pane", color: "fg" },
      ring: glyph
        ? { borderColor: "accent" }
        : { borderColor: "accent", color: "fg" },
      onClick,
    },
    [
      glyph ? ui.icon(glyph, { size: 11, color: "fgMuted" }) : null,
      text.toUpperCase(),
    ],
  );

// A square button at the end of the field (`.send`): `mode` is `idle`
// (nothing to send yet), `run` (send, or queue), `stop` or `force`. Ready,
// the primary colour fills it over a spring, and its glyph arrives with it.
function sendButton(id, glyph, mode, m, onClick) {
  const ready = mode === "run";
  const frame = {
    idle: "line",
    run: "primaryRim",
    stop: "mix(line, error, 0.6)",
    force: "mix(lineStrong, accent, 0.55)",
  }[mode];
  const ink = {
    idle: "fgFaint",
    run: "primaryInk",
    stop: "error",
    force: "accent",
  }[mode];
  const hover =
    mode === "run"
      ? "mix(primary, fg, 0.15)"
      : mode === "stop"
        ? "alpha(error, 0.09)"
        : "alpha(accent, 0.12)";
  const live = mode !== "idle";
  return ui.row(
    {
      id,
      key: id,
      position: "relative",
      shrink: 0,
      w: 28,
      h: 28,
      radius: m.radius,
      border: "1px",
      borderColor: frame,
      bg: "pane",
      items: "center",
      justify: "center",
      cursor: live ? "pointer" : undefined,
      hover: live ? { bg: hover } : undefined,
      // Pressed, it gives a pixel (`.send:active`).
      active: live ? { top: 1 } : undefined,
      ring: live ? { borderColor: ready ? "primaryInk" : "accent" } : undefined,
      onClick: live ? onClick : undefined,
    },
    [
      // The primary colour as a layer over the resting button, rather than
      // a colour that would pass through other hues on its way.
      ui.box({
        id: id + "-fill",
        position: "absolute",
        top: 0,
        left: 0,
        w: "full",
        h: "full",
        radius: m.radius - 1,
        bg: "primary",
        opacity: 0,
        spring: { on: ready, opacity: 1 },
      }),
      // (An even glyph in an even button: centred on whole pixels.)
      ui.icon(glyph, {
        size: 14,
        color: ink,
        enter: {
          key: id + "-glyph-" + ready,
          ms: 120,
          easing: "ease-out",
          size: 11,
          opacity: 0,
        },
      }),
    ],
  );
}

// The buttons at the end of the field: send; while the agent works, stop,
// or force and queue once something is typed. Always two places, the first
// empty but for force, so the row keeps its shape.
function buttons(c, m) {
  const empty = ui.box({ shrink: 0, w: 0, h: 0 });
  const row = (first, second) =>
    ui.row({ shrink: 0, items: "center", gap: 8 }, [first, second]);
  if (c.buttons === "stop") {
    return row(
      empty,
      sendButton("stop", "stop", "stop", m, (api) => api.act("field.stop")),
    );
  }
  if (c.buttons === "force") {
    return row(
      ui.box({ id: "force", shrink: 0 }, [
        sendButton("force", "bolt", "force", m, (api) =>
          api.act("field.force"),
        ),
      ]),
      sendButton("queue", "queue", "run", m, (api) => api.act("field.send")),
    );
  }
  return row(
    empty,
    sendButton("send", "play", c.ready ? "run" : "idle", m, (api) =>
      api.act("field.send"),
    ),
  );
}

// The model's, the mode's or the effort's control (`.model-pick`, `.think`):
// framed, what is chosen and that it opens. What it chooses among is said by
// its list, which is headed.
function picker(kind, chosen, d, m, slot) {
  let inner;
  if (chosen.effort) {
    // kou's own agent offers its efforts as modes, another agent may offer
    // them apart: bars filled to the level.
    const { level, bars } = chosen.effort;
    inner = [
      ui.row(
        { shrink: 0, h: 17, items: "end", gap: 3 },
        Array.from({ length: bars }, (_, bar) =>
          ui.box({
            w: 5,
            h: 5 + 3 * bar,
            radius: m.radiusMark ?? 2,
            bg: bar < level ? "accent" : "lineStrong",
          }),
        ),
      ),
      label(chosen.name, "fgDim"),
    ];
  } else {
    inner = [
      kind === "model"
        ? ui.box({ shrink: 0, w: 7, h: 7, radius: "full", bg: chosen.dot })
        : null,
      ui.text(chosen.name, {
        minW: 0,
        truncate: true,
        font: "mono",
        weight: "semibold",
        size: 12,
        color: "fg",
      }),
      ui.icon("chevron-down", { size: 12, color: "fgMuted" }),
    ];
  }
  return ui.box({ position: "relative", minW: 0 }, [
    // (An app that still puts the model picker over the field itself hands
    // no `modelMenu`.)
    slot(kind + "Menu"),
    ui.row(
      {
        id: kind + "-picker",
        key: kind + "-picker",
        h: 26,
        minW: 0,
        maxW: 260,
        pl: 9,
        pr: chosen.effort ? 9 : 6,
        items: "center",
        gap: 6,
        radius: m.radius,
        border: "1px",
        borderColor: chosen.open ? "lineStrong" : "line",
        bg: chosen.open ? "hover" : "pane",
        cursor: "pointer",
        hover: { bg: "hover", borderColor: "lineStrong" },
        ring: { borderColor: "accent" },
        onClick: (api) => api.act("field." + kind),
      },
      inner,
    ),
  ]);
}

kou.view({
  id: "composer",
  render(ctx) {
    if (ctx.data.look === "studio") return studioComposer(ctx);
    const d = ctx.data;
    const m = ctx.theme.metric;
    const slot = (name) => (d.slots.includes(name) ? ui.slot(name) : null);
    const c = d.controls;
    // The foot: what the app puts there, then attach, the model, the level
    // of effort and the mode; the buttons at its end.
    const foot = c
      ? [
          slot("foot"),
          c.attach
            ? ui.iconButton("paperclip", {
                id: "attach",
                key: "attach",
                size: 26,
                onClick: (api) => api.act("field.attach"),
              })
            : null,
          c.model ? picker("model", c.model, d, m, slot) : null,
          c.effort ? picker("effort", c.effort, d, m, slot) : null,
          c.mode ? picker("mode", c.mode, d, m, slot) : null,
          c.access ? picker("access", c.access, d, m, slot) : null,
        ]
      : [
          slot("foot"),
          slot("attach"),
          slot("model"),
          slot("effort"),
          slot("mode"),
          slot("access"),
        ];
    return ui.box(
      {
        id: "frame",
        position: "relative",
        radius: m.radiusLarge,
        bg: "raised",
        pl: 14,
        pr: 10,
        pt: 12,
        pb: 10,
        // Files dragged over it: it says it takes them.
        drop: { bg: "selected" },
      },
      [
        slot("editor"),
        ui.row({ mt: 10, items: "center", gap: 8 }, [
          ui.row({ grow: 1, minW: 0, items: "center", gap: 8 }, foot),
          c ? buttons(c, m) : slot("controls"),
        ]),
      ],
    );
  },
});

// The line over the field while the agent works (`.activity`): the meter
// (`ui.slot("meter")`, which moves on its own), what the agent does, the
// agents it waits on and the commands it left running, and how to stop it.
// `ctx.data`: `blocked`, `queued` (the queue stands under it), `doing`,
// `waiting` (the agents' titles), `background`, `commands`, `hint` (the
// key that stops it), `stop` (a button that does).
kou.view({
  id: "composer-activity",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    return ui.row(
      {
        id: "activity",
        h: 32,
        // Over the queue's tray it keeps to that tray's edges.
        px: d.queued ? 9 + m.radiusLarge : 2,
        items: "center",
        gap: 10,
        font: "ui",
        size: 12,
        color: d.blocked ? "blocked" : "fgDim",
      },
      [
        d.slots.includes("meter")
          ? ui.box({ id: "activity-meter", shrink: 0 }, [ui.slot("meter")])
          : null,
        d.doing ? ui.text(d.doing, { minW: 0, truncate: true }) : null,
        d.waiting
          ? ui.text(d.waiting, {
              id: "waiting-titles",
              minW: 0,
              shrink: 1,
              truncate: true,
              color: "fgMuted",
            })
          : null,
        d.background
          ? ui.row(
              {
                id: "activity-background",
                minW: 0,
                items: "center",
                gap: 6,
                color: "fgDim",
              },
              [
                ui.icon("terminal", { size: 12, color: "fgMuted" }),
                ui.text(d.background, { minW: 0, truncate: true }),
              ],
            )
          : null,
        d.commands
          ? ui.text(d.commands, {
              font: "mono",
              minW: 0,
              shrink: 1,
              truncate: true,
              color: "fgMuted",
            })
          : null,
        ui.box({ grow: 1 }),
        d.hint
          ? ui.row(
              {
                shrink: 0,
                items: "center",
                gap: 4,
                size: 10.5,
                color: "fgMuted",
              },
              [
                key("esc", m),
                // (Twice: one press only asks.)
                "twice to stop",
              ],
            )
          : null,
        d.stop
          ? act("waiting-stop", "Stop", m, (api) => api.act("stop"))
          : null,
      ],
    );
  },
});

// The queue over the field (`.queue`): a tray on the field's straight edge,
// clear of its corners, with its head (how many, what the queue does, its
// tools) over the rows, which stay the app's own (`ui.slot("rows")`): they
// take the keys and are dragged to another place. `ctx.data`: `count`,
// `state`, `waiting` (nothing runs them now), `paused`, `queued` (how many
// wait to be sent). Its tools ask `resume`, `pause` and `clear`.
kou.view({
  id: "composer-queue",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    return ui.box(
      {
        id: "queue",
        mx: 9 + m.radiusLarge,
        mb: -1,
        radiusT: m.radius,
        border: "1px",
        borderColor: d.paused ? "mix(lineStrong, blocked, 0.45)" : "lineStrong",
        bg: "pane",
        overflow: "hidden",
      },
      [
        ui.row(
          {
            h: 32,
            pl: 12,
            pr: 6,
            items: "center",
            gap: 10,
            font: "mono",
            size: 11,
          },
          [
            ui.text(String(d.count), {
              id: "queue-count",
              minW: 18,
              h: 16,
              px: 4,
              radius: m.radiusSmall,
              dir: "row",
              items: "center",
              justify: "center",
              bg: d.waiting ? "blocked" : "accent",
              color: "accentInk",
              weight: "bold",
              size: 10,
            }),
            ui.text(d.state, {
              minW: 0,
              grow: 1,
              truncate: true,
              color: d.paused ? "blocked" : "fgMuted",
            }),
            d.queued > 0 && d.waiting
              ? act(
                  "queue-resume",
                  "Resume",
                  m,
                  (api) => api.act("resume"),
                  "play",
                )
              : null,
            d.queued > 0 && !d.waiting
              ? act(
                  "queue-pause",
                  "Pause",
                  m,
                  (api) => api.act("pause"),
                  "pause",
                )
              : null,
            d.queued > 0
              ? act(
                  "queue-clear",
                  "Clear",
                  m,
                  (api) => api.act("clear"),
                  "close",
                )
              : null,
          ],
        ),
        ui.box(
          {
            position: "relative",
            w: "full",
            h: d.count * 36,
            borderT: "1px",
            borderColor: "line",
          },
          [ui.slot("rows")],
        ),
      ],
    );
  },
});

// The figures under the field (`.stats-dock`): what the agent reported of
// its context, tokens and cost, a dot between. `ctx.data.figures`:
// `{ key, value, share }`, `share` (of the context) for a meter.
kou.view({
  id: "composer-usage",
  render(ctx) {
    const meter = (share) => {
      const shown = Math.min(1, Math.max(0, share));
      const tone =
        share >= 0.9 ? "error" : share >= 0.75 ? "blocked" : "accent";
      return ui.box(
        { shrink: 0, w: 22, h: 4, radius: "full", bg: "lineStrong" },
        [ui.box({ h: "full", w: 22 * shown, radius: "full", bg: tone })],
      );
    };
    return ui.row(
      {
        id: "usage",
        minH: 22,
        pl: 4,
        pr: 4,
        wrap: true,
        items: "center",
        font: "mono",
        size: 10.5,
        lineHeight: 22,
      },
      ctx.data.figures.map((figure, at) =>
        ui.row(
          { position: "relative", shrink: 0, items: "center", gap: 6, px: 9 },
          [
            at > 0
              ? ui.box({
                  position: "absolute",
                  left: -1.5,
                  w: 3,
                  h: 3,
                  radius: "full",
                  bg: "lineStrong",
                })
              : null,
            ui.text(figure.key, {
              weight: "semibold",
              size: 9.5,
              color: "fgMuted",
            }),
            figure.share === null || figure.share === undefined
              ? null
              : meter(figure.share),
            ui.text(figure.value, { weight: "semibold", color: "fgMuted" }),
          ],
        ),
      ),
    );
  },
});

function studioComposer(ctx) {
  const d = ctx.data,
    c = d.controls,
    m = ctx.theme.metric;
  const slot = (name) => (d.slots.includes(name) ? ui.slot(name) : null);
  const control = (id, glyph, enabled, neutral = false) =>
    ui.row(
      {
        id,
        key: id,
        w: 27.6,
        h: 27.6,
        shrink: 0,
        radius: "full",
        items: "center",
        justify: "center",
        bg: neutral ? "selected" : "primary",
        color: neutral ? "fgDim" : "primaryInk",
        border: "1px",
        borderColor: "transparent",
        focusable: enabled,
        cursor: enabled ? "pointer" : undefined,
        hover: enabled
          ? { bg: neutral ? "hover" : "mix(primary, fg, 0.16)" }
          : undefined,
        ring: enabled ? { borderColor: "accent" } : undefined,
        onClick: enabled
          ? (api) => api.act("field." + (id === "queue" ? "send" : id))
          : undefined,
      },
      [
        ui.icon(glyph, {
          size: 14,
          color: neutral ? "fgDim" : "primaryInk",
          opacity: enabled ? 1 : 0.85,
        }),
      ],
    );
  const pick = (kind, choice) =>
    choice &&
    ui.box({ position: "relative", minW: 0 }, [
      ui.box({ position: "absolute", top: 0, right: 0 }, [slot(kind + "Menu")]),
      ui.row(
        {
          id: kind + "-picker",
          key: kind,
          h: 27.6,
          minW: 0,
          maxW: d.narrow ? 120 : 240,
          px: 2,
          gap: 6,
          items: "center",
          radius: m.radius,
          border: "1px",
          borderColor: "transparent",
          cursor: "pointer",
          hover: { bg: "hover" },
          ring: { borderColor: "accent" },
          onClick: (api) => api.act("field." + kind),
        },
        [
          choice.dot &&
            ui.box({ w: 7, h: 7, shrink: 0, radius: "full", bg: choice.dot }),
          ui.text(choice.name.replace(/^Effort: /, ""), {
            minW: 0,
            truncate: true,
            size: 13.54,
            color: kind === "model" ? "alpha(fg, 0.85)" : "fgDim",
          }),
        ],
      ),
    ]);
  return ui.col(
    {
      id: "frame",
      position: "relative",
      minH: 100,
      radius: m.radiusLarge,
      bg: "raised",
      pl: 9.6,
      pr: 6.4,
      pt: 9.6,
      pb: 7.8,
      drop: { bg: "selected" },
    },
    [
      ui.box({ pr: 3.2 }, [slot("editor")]),
      ui.box({ grow: 1, minH: 6 }),
      ui.row({ h: 27.6, items: "center", gap: 8 }, [
        ui.row({ grow: 1, minW: 0, items: "center", gap: 8 }, [
          c.attach &&
            ui.row(
              {
                id: "attach",
                key: "attach",
                w: 24,
                h: 24,
                ml: -2.6,
                radius: "full",
                shrink: 0,
                border: "1px",
                borderColor: "transparent",
                items: "center",
                justify: "center",
                cursor: "pointer",
                tooltip: "Attach files",
                hover: { bg: "hover" },
                ring: { borderColor: "accent" },
                onClick: (api) => api.act("field.attach"),
              },
              [ui.icon("paperclip", { size: 17, color: "fgMuted" })],
            ),
          ui.box({ id: "field-foot", minW: 0 }, [slot("foot")]),
        ]),
        pick("model", c.model),
        pick("effort", c.effort),
        pick("mode", c.mode),
        pick("access", c.access),
        ui.row({ shrink: 0, items: "center", gap: 6 }, [
          c.buttons === "force" && control("force", "bolt", true, true),
          control(
            c.buttons === "force" ? "queue" : "send",
            "arrow-up",
            c.buttons === "force" || c.ready,
          ),
        ]),
      ]),
    ],
  );
}
