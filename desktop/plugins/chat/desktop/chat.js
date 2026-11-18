// A chat's transcript: messages, tool calls and their details.
//
// Streamed text and animation are placed with `ui.slot(name)`: what was said (the
// prose, a prompt's text and pictures), what a call gave and the end of
// what a command prints while it runs, a mark that blinks, the time of a
// call that runs, a light going round a card. `ctx.data.slots` names the
// ones an entry has now. Nothing in `ctx.data` streams: an entry is drawn
// again when how it stands changes, not as words come.
//
// An entry asks the chat with `api.act`: `toggle` (`{ index, open,
// keyboard }`, open or close it), `worked` (`{ start, keyboard }`, a run of
// work), `permit` (`{ request, option }`, answer what a call asks),
// `choose` (`{ request, option, keyboard }`, an option of a question it asks,
// by its place), `confirm` and `back` (`{ request, keyboard }`), `copy`,
// `edit` and `reuse` (`{ index }`), `unclear` (show what `/clear` hid),
// `unfold` (`{ index, piece }`, keep more of what a command printed),
// `nested` (`{ id, keyboard }`, open an agent started in a pane), `show`
// (`{ index, keyboard }`, show the card of the agent a call started).
//
// The agents' cards are here (`chat-agent`, `chat-nested`), and so are the
// card beside the transcript (`chat-beside`, which asks `folder` and
// `beside`, `{ open }`; `chat-beside-handle`) and the ticks down its edge
// (`chat-ticks`, which ask `tick`, `{ index }`).
//
// The lists and the card over the field are here too (`chat-menu`,
// `chat-choices` and `chat-choice`, `chat-models` and `chat-model-line`,
// `chat-help`): their cards, heads, lines and feet. What takes the keys or
// the pointer and what moves stays the field's (the highlight, each line's
// place, the search field, the model list that scrolls and slides), and so
// do where a card stands and how it rises.

// Height of the label above a message.
const META = 22;

const has = (d, name) => d.slots.includes(name);
const slot = (d, name) => (has(d, name) ? ui.slot(name) : null);
// Borders are `"1px"`: a hairline at every zoom, as the app's own are; a
// width in numbers grows and shrinks with a chat on the canvas.
//
// Whether a click came from the keys (they then stay where they are).
const keyed = (event) => Boolean(event && event.keyboard);

// The web cockpit's `.label`: a word in capitals of the monospace.
const label = (text, color) =>
  ui.text(String(text).toUpperCase(), { shrink: 0, font: "mono", weight: "bold", size: 10.5, color });

// The line of capitals over an entry: who, then what is known of it.
const metaLine = (children) => ui.row({ h: META, w: "full", items: "center", gap: 10 }, children);

// Small monospace text: a path, a count.
const meta = (text, props) => ui.text(text, { font: "mono", size: 11, color: "fgMuted", ...props });

// A fold's chevron: right when closed, down when open.
const chevron = (open) => ui.icon(open ? "chevron-down" : "chevron-right", { size: 12, color: "fgFaint" });

// A key, as the app shows one to press.
const key = (keys, m) =>
  ui.text(keys, {
    shrink: 0,
    px: 4,
    radius: m.radiusSmall,
    border: "1px",
    borderColor: "lineStrong",
    font: "mono",
    size: 10.5,
    color: "fgMuted",
  });

// A control that shows with the pointer over its entry (`group`) or the
// keys on it (`.actions`): copy, edit.
const revealed = (id, text, group, m, onClick, copied) =>
  ui.text(text.toUpperCase(), {
    id,
    key: id,
    shrink: 0,
    h: 22,
    px: 7,
    radius: m.radius,
    border: "1px",
    borderColor: "transparent",
    dir: "row",
    items: "center",
    cursor: "pointer",
    font: "mono",
    weight: "semibold",
    size: 10,
    color: copied ? "ok" : "fgMuted",
    opacity: copied ? 1 : 0,
    hover: { borderColor: "lineStrong", bg: "pane", color: "fg", opacity: 1 },
    ring: { borderColor: "accent", color: "fg", opacity: 1 },
    groupHover: { group, opacity: 1 },
    onClick,
  });

// The line over an answer: who, and a control to copy all of it once it
// is all said.
kou.view({
  id: "chat-answer-head",
  render(ctx) {
    const d = ctx.data;
    const copy = (api, event) => api.act("copy", { index: d.index, keyboard: keyed(event) });
    // (Its control at the end of the line: the line's ends apart.)
    return ui.row({ h: META, w: "full", items: "center", justify: "between", gap: 10 }, [
      label("Agent", "fg"),
      d.writing
        ? null
        : revealed("copy-answer-" + d.index, d.copied ? "Copied" : "Copy", "answer-" + d.index, ctx.theme.metric, copy, d.copied),
    ]);
  },
});

// The line over a prompt: who, the model it went to (`model`: `{ id, dot }`),
// and what can be done with it (`action`: `edit`, `reuse` or none).
kou.view({
  id: "chat-user-head",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const group = "user-" + d.index;
    const copy = (api, event) => api.act("copy", { index: d.index, keyboard: keyed(event) });
    return metaLine([
      label("You", "accent"),
      d.model
        ? ui.row(
            {
              shrink: 0,
              maxW: 260,
              h: 18,
              px: 6,
              items: "center",
              gap: 6,
              radius: m.radiusSmall,
              border: "1px",
              borderColor: "line",
              font: "mono",
              size: 11,
              color: "fgMuted",
            },
            [
              ui.box({ shrink: 0, w: 6, h: 6, radius: m.radiusMark, bg: d.model.dot }),
              ui.text(d.model.id, { minW: 0, truncate: true }),
            ],
          )
        : null,
      // What can be done with it, at the end of the line: the rest of the
      // line is theirs (as a gap and a box that takes the room were).
      ui.row({ grow: 1, shrink: 0, pl: 10, justify: "end", gap: 2 }, [
        revealed("copy-prompt-" + d.index, d.copied ? "Copied" : "Copy", group, m, copy, d.copied),
        d.action
          ? revealed(
              d.action + "-prompt-" + d.index,
              d.action === "edit" ? "Edit" : "Reuse",
              group,
              m,
              (api) => api.act(d.action, { index: d.index }),
            )
          : null,
      ]),
    ]);
  },
});

// A prompt's card: what was typed (`text`), its pictures (`pictures`), and
// the files it brought (`files`).
kou.view({
  id: "chat-user-card",
  render(ctx) {
    const d = ctx.data;
    return ui.col(
      {
        w: "full",
        px: 15,
        pt: 10,
        pb: 11,
        radius: ctx.theme.metric.radiusLarge,
        border: "1px",
        borderColor: "line",
        bg: "raised",
        size: 14.5,
        lineHeight: 24,
        gap: 2,
      },
      [
        slot(d, "text"),
        slot(d, "pictures"),
        d.files.map((path) =>
          ui.row({ font: "mono", size: 11, color: "fgMuted", items: "center", gap: 6 }, [
            ui.icon("file", { size: 12, color: "fgMuted" }),
            ui.text(path, { minW: 0, truncate: true, color: "fgDim" }),
          ]),
        ),
      ],
    );
  },
});

// Reasoning: a line that says what it is about and opens on it. While it
// is written, the label under its light (`who`) and what it is about now
// (`gist`) are the app's; open, so is the reasoning (`prose`).
kou.view({
  id: "chat-thought",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const fold = ui.row(
        {
          id: "thought-" + d.index,
          key: "fold",
          h: META,
          w: "full",
          items: "center",
          gap: 10,
          radius: m.radius,
          border: "1px",
          borderColor: "transparent",
          cursor: "pointer",
          color: "fgMuted",
          hover: { color: "fgDim" },
          ring: { borderColor: "accent" },
          onClick: (api, event) =>
            api.act("toggle", { index: d.index, open: !d.open, keyboard: keyed(event) }),
        },
        [
          has(d, "who") ? ui.slot("who") : label("Thinking", "fgMuted"),
          has(d, "gist")
            ? ui.slot("gist")
            : ui.text(d.gist || "", { minW: 0, grow: 1, truncate: true, italic: true }),
          chevron(d.open),
        ],
      );
    if (!d.open) return fold;
    // `.prose.thought`: a dashed rule beside it, a step back from the answer.
    return ui.box({ w: "full" }, [
      fold,
      ui.box({ mt: 8, pl: 14, borderL: "1px", dashed: true, borderColor: "lineStrong" }, [slot(d, "prose")]),
    ]);
  },
});

// The state of a tool call, as a mark of its own shape: a square while it
// runs (the app's, `glyph`, as it blinks), a tick done, a cross failed.
//
// Each stands in the middle of a box 14 across, as the chat's own does: by
// margins of its own instead, it lands a screen pixel aside at some zooms
// of a canvas.
function glyph(d, m) {
  const square = (color, filled) =>
    ui.box({ w: 8, h: 8, radius: m.radiusMark, ...(filled ? { bg: color } : { border: "1px", borderColor: color }) });
  const shapes = {
    asking: () => square("blocked", true),
    working: () => square("working", true),
    pending: () => square("fgMuted", false),
    done: () => ui.icon("check", { size: 12, color: "ok" }),
    failed: () => ui.icon("close", { size: 12, color: "error" }),
    cancelled: () => ui.box({ w: 9, h: 9, radius: "full", border: "1px", borderColor: "fgMuted" }),
  };
  const shape = shapes[d.glyph];
  if (!shape) return slot(d, "glyph");
  return ui.row({ shrink: 0, w: 14, items: "center", justify: "center" }, [shape()]);
}

// The answers to a call that waits for the user's (`.tool-ask`): a button
// for each, the usual one filled, and the key that gives it.
function answers(a, m) {
  const button = (option, at) =>
    ui.text(option.label.toUpperCase(), {
      id: "answer-" + option.id,
      key: "answer-" + at,
      shrink: 0,
      h: 28,
      px: 12,
      radius: m.radius,
      border: "1px",
      dir: "row",
      items: "center",
      cursor: "pointer",
      font: "mono",
      weight: "bold",
      size: 10.5,
      ...(option.primary
        ? {
            bg: "primary",
            borderColor: "primaryRim",
            color: "primaryInk",
            hover: { bg: "mix(fg, primary, 0.15)" },
            ring: { borderColor: "primaryInk" },
          }
        : {
            borderColor: "lineStrong",
            color: "fgDim",
            hover: { borderColor: "fgFaint", color: "fg", bg: "pane" },
            ring: { borderColor: "accent" },
          }),
      active: { opacity: 0.85 },
      onClick: (api) => api.act("permit", { request: a.request, option: option.id }),
    });
  return ui.row(
    {
      w: "full",
      px: 12,
      py: 10,
      borderT: "1px",
      borderColor: "alpha(blocked, 0.35)",
      bg: "alpha(blocked, 0.06)",
      wrap: true,
      items: "center",
      gap: 6,
    },
    [
      a.options.map(button),
      a.hint
        ? ui.row({ ml: 4, items: "center", gap: 5, font: "mono", size: 10.5, color: "fgMuted" }, [key(a.key, m), a.hint])
        : null,
    ],
  );
}

// The questions a call asks the user (`asks`), in the band where a plain
// request has its buttons: what was answered before the one at hand
// (`answered`, `{ name, answer }` a line each); the one at hand, `at` of
// `count` (from 0), under its `header`, with its `text`; its `options`
// (`label`, `description`, `chosen` of one that takes several, `key` the
// digit that chooses it) as rows; the line that says words can be typed in
// the field (`words`); `back`; `confirm` (null, or whether it is live) and
// the ways to refuse (`refusals`, `{ id, label }`), as the request's
// buttons are. A row asks `choose`, Confirm `confirm`, Back `back`, a
// refusal `permit`.
function asks(q, m) {
  const request = q.request;
  // The plan's mark of a step: 14 wide, the mark in its middle.
  const cell = (props, child) =>
    ui.row({ shrink: 0, w: 14, items: "center", justify: "center", ...props }, [child]);
  const answered = q.answered.map((line, at) =>
    ui.row({ key: "answered-" + at, h: META, items: "center", gap: 10, font: "mono", size: 11 }, [
      cell({}, ui.icon("check", { size: 12, color: "ok" })),
      ui.text(line.name, { minW: 0, shrink: 1, truncate: true, color: "fgMuted" }),
      ui.text(line.answer, { minW: 0, shrink: 1, truncate: true, color: "fgDim" }),
    ]),
  );
  const head = ui.row({ h: META, items: "center", gap: 10 }, [
    label(q.header || "Question", "fgDim"),
    q.count > 1 ? meta(q.at + 1 + " of " + q.count, { shrink: 0 }) : null,
    ui.box({ grow: 1 }),
    q.back
      ? act("ask-back", "Back", m, (api, event) =>
          api.act("back", { request, keyboard: keyed(event) }),
        )
      : null,
  ]);
  const option = (o, at) =>
    ui.row(
      {
        id: "ask-option-" + at,
        key: "ask-option-" + at,
        // Its frame lies outside the column, its words in it.
        mx: -7,
        px: 6,
        items: "start",
        gap: 10,
        radius: m.radius,
        border: "1px",
        borderColor: "transparent",
        cursor: "pointer",
        hover: { bg: "hover" },
        ring: { borderColor: "accent" },
        onClick: (api, event) =>
          api.act("choose", { request, option: at, keyboard: keyed(event) }),
      },
      [
        cell(
          { h: 22 },
          o.chosen
            ? ui.icon("check", { size: 12, color: "accent" })
            : ui.box({ w: 8, h: 8, radius: m.radiusMark, border: "1px", borderColor: "fgFaint" }),
        ),
        ui.box({ grow: 1, minW: 0 }, [
          ui.text(o.label, { size: 13.5, lineHeight: 22, color: "fg" }),
          o.description
            ? ui.text(o.description, { pb: 4, font: "mono", size: 11, lineHeight: 16, color: "fgMuted" })
            : null,
        ]),
        o.key ? ui.row({ shrink: 0, h: 22, items: "center" }, [key(o.key, m)]) : null,
      ],
    );
  const words = q.words
    ? ui.row({ h: 22, items: "center", gap: 10 }, [
        cell({}, ui.icon("keyboard", { size: 14, color: "fgMuted" })),
        ui.text(q.words, { minW: 0, truncate: true, font: "mono", size: 11, color: "fgMuted" }),
      ])
    : null;
  const button = (id, text, primary, onClick) =>
    ui.text(text.toUpperCase(), {
      id,
      key: id,
      shrink: 0,
      h: 28,
      px: 12,
      radius: m.radius,
      border: "1px",
      dir: "row",
      items: "center",
      cursor: "pointer",
      font: "mono",
      weight: "bold",
      size: 10.5,
      ...(primary
        ? {
            bg: "primary",
            borderColor: "primaryRim",
            color: "primaryInk",
            hover: { bg: "mix(fg, primary, 0.15)" },
            ring: { borderColor: "primaryInk" },
          }
        : {
            borderColor: "lineStrong",
            color: "fgDim",
            hover: { borderColor: "fgFaint", color: "fg", bg: "pane" },
            ring: { borderColor: "accent" },
          }),
      active: { opacity: 0.85 },
      onClick,
    });
  // Confirm while nothing is chosen or typed: there, but not to be pressed,
  // as the field's own button is with nothing to send.
  const unready = () =>
    ui.text("CONFIRM", {
      id: "ask-confirm",
      shrink: 0,
      h: 28,
      px: 12,
      radius: m.radius,
      border: "1px",
      borderColor: "line",
      dir: "row",
      items: "center",
      font: "mono",
      weight: "bold",
      size: 10.5,
      color: "fgFaint",
    });
  const confirm = typeof q.confirm === "boolean";
  const live = q.confirm === true;
  const buttons =
    confirm || q.refusals.length > 0
      ? ui.row({ mt: 8, wrap: true, items: "center", gap: 6 }, [
          !confirm
            ? null
            : live
              ? button("ask-confirm", "Confirm", true, (api, event) =>
                  api.act("confirm", { request, keyboard: keyed(event) }),
                )
              : unready(),
          q.refusals.map((refusal) =>
            button("answer-" + refusal.id, refusal.label, false, (api) =>
              api.act("permit", { request, option: refusal.id }),
            ),
          ),
          live
            ? ui.row({ ml: 4, items: "center", gap: 5, font: "mono", size: 10.5, color: "fgMuted" }, [
                key("enter", m),
                "confirm",
              ])
            : null,
        ])
      : null;
  return ui.col(
    {
      w: "full",
      px: 12,
      py: 10,
      borderT: "1px",
      borderColor: "alpha(blocked, 0.35)",
      bg: "alpha(blocked, 0.06)",
    },
    [
      answered,
      head,
      ui.text(q.text, { size: 13.5, lineHeight: 22, color: "fg" }),
      ui.col({ mt: 4, gap: 2 }, [q.options.map(option), words]),
      buttons,
    ],
  );
}

// A card's border by how its call or agent goes (`border`).
const BORDERS = {
  blocked: "mix(line, blocked, 0.45)",
  working: "mix(line, working, 0.5)",
  error: "mix(line, error, 0.45)",
  line: "line",
};

// A tool call's card (`.tool`): its head shows the action and input
// and how it went; under it what it gave (`body`), or,
// closed while it runs, the end of what it prints (`live`); and the answers
// it waits for, or the questions it asks (`asks`). Its border says how it
// goes.
kou.view({
  id: "chat-tool",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const quiet = d.glyph === "done" && d.border === "line" && !d.open && !has(d, "body");
    const toggle = (api, event) =>
      api.act("toggle", { index: d.index, open: !d.open, keyboard: keyed(event) });
    const head = ui.row(
      {
        id: "tool-" + d.index,
        key: "call",
        h: m.control,
        px: 6,
        items: "center",
        gap: 6,
        border: "1px",
        borderColor: "transparent",
        radius: m.radiusMedium - 1,
        font: "mono",
        size: 12,
        ...(d.expandable
          ? { cursor: "pointer", hover: { bg: "hover" }, ring: { borderColor: "accent" }, onClick: toggle }
          : {}),
      },
      [
        glyph(d, m),
        ui.text(d.verb, { shrink: 0, font: "ui", size: 12, weight: "medium", color: "fgDim" }),
        d.input ? ui.text(d.input, { minW: 0, shrink: 1, truncate: true, color: "fg" }) : null,
        // The path gives way before what the call was given does.
        d.location ? meta(d.location, { minW: 0, truncate: "start", shrink: 1000 }) : null,
        // What changed, how long it took, how it ended: at the end of the
        // line, aligned to its right edge.
        ui.row({ font: "mono", size: 11, color: "fgMuted", grow: 1, shrink: 0, justify: "end", items: "center", gap: 6 }, [
          // (Closer to each other than the rest are, in a row of their own:
          // a gap taken back by a margin rounds apart from the chat's own
          // at some zooms of a canvas.)
          d.added + d.removed > 0
            ? ui.row({ gap: 5 }, [
                d.added > 0 ? ui.text("+" + d.added, { color: "ok" }) : null,
                d.removed > 0 ? ui.text("-" + d.removed, { color: "error" }) : null,
              ])
            : null,
          slot(d, "took"),
          d.exit !== null && d.exit !== undefined ? ui.text("Exit " + d.exit, { color: "blocked" }) : null,
          d.word ? ui.text(d.word.text, { font: "ui", weight: "medium", size: 12, color: d.word.tone }) : null,
        ]),
        d.expandable ? chevron(d.open) : null,
      ],
    );
    return ui.box(
      { position: "relative", w: "full", radius: m.radiusMedium, border: "1px", borderColor: quiet ? "transparent" : BORDERS[d.border], bg: quiet ? "transparent" : "raised", overflow: "hidden" },
      [
        head,
        slot(d, "body"),
        slot(d, "live"),
        // (`asks` is not said by an older app: its answers are drawn then.)
        d.asks ? asks(d.asks, m) : d.answers ? answers(d.answers, m) : null,
        // Its time counting while it runs in the background, and its light.
        slot(d, "counting"),
        slot(d, "spot"),
      ],
    );
  },
});

// A section's head in a call's card (`.stream header`): what it is, in
// capitals, and what stands at its end.
const sectionHead = (name, end) =>
  ui.row(
    { h: 24, pl: 12, pr: 10, items: "center", justify: "between", gap: 10, font: "mono", weight: "bold", size: 10, color: "fgMuted" },
    [ui.text(name.toUpperCase(), { minW: 0, truncate: true }), end],
  );

// A piece of what a call gave, by its `kind`: a change to a file (`diff`:
// its `path`, lines `added` and `removed`, `empty`, and how many of its
// lines are left out, `leftOut`); what a command printed (`terminal`:
// `running`, and how many earlier lines are `hidden`, `unfolded` once more
// are kept, which a click on that line asks with `unfold`); or what went in
// or came back as written (`raw`, its `label`). The lines themselves are the
// app's (`lines`).
kou.view({
  id: "chat-tool-piece",
  render(ctx) {
    const d = ctx.data;
    if (d.kind === "diff") {
      return ui.box({ w: "full" }, [
        sectionHead(
          "Change",
          ui.row({ minW: 0, items: "center", gap: 10, weight: "normal", size: 11 }, [
            ui.text(d.path, { minW: 0, truncate: "start", color: "fgDim" }),
            ui.text(d.empty ? "no changes" : "+" + d.added + " -" + d.removed, { shrink: 0 }),
          ]),
        ),
        slot(d, "lines"),
        d.leftOut > 0
          ? ui.text(d.leftOut + " more lines are not shown", {
              px: 12,
              h: 28,
              dir: "row",
              items: "center",
              borderT: "1px",
              dashed: true,
              borderColor: "line",
              font: "mono",
              size: 11,
              color: "fgMuted",
            })
          : null,
      ]);
    }
    if (d.kind === "terminal") {
      const more = d.unfolded
        ? d.hidden + " earlier lines are not kept on screen"
        : "Show " + d.hidden + " earlier lines";
      return ui.box({ w: "full" }, [
        sectionHead("Output", d.running ? ui.text("RUNNING", { shrink: 0, color: "working" }) : null),
        d.hidden > 0
          ? ui.text(more, {
              id: "unfold-" + d.index + "-" + d.piece,
              key: "unfold",
              px: 12,
              h: 22,
              dir: "row",
              items: "center",
              border: "1px",
              borderColor: "transparent",
              font: "mono",
              size: 11,
              color: "fgMuted",
              ...(d.unfolded
                ? {}
                : {
                    cursor: "pointer",
                    hover: { bg: "hover" },
                    ring: { borderColor: "accent" },
                    onClick: (api) => api.act("unfold", { index: d.index, piece: d.piece }),
                  }),
            })
          : null,
        slot(d, "lines"),
      ]);
    }
    return ui.box({ w: "full" }, [sectionHead(d.label), slot(d, "lines")]);
  },
});

// The line under a card's head with an agent's latest words, which are the
// app's (the slot `name`): they change as the agent works.
const latestLine = (name, props) =>
  ui.box({ pl: 36, pr: 12, pb: 6, truncate: true, font: "mono", size: 11.5, color: "fgMuted", ...props }, [
    ui.slot(name),
  ]);

// The card of a call that starts a subagent: its harness's tile (`code`),
// what it was started for (`title`), its kind and counts (`about`), how long
// it has been at it (the app's, `took`) and how it goes (`word` in `tone`);
// closed, its latest words; open, what it was asked and did (`body`); and
// the answers it waits for.
kou.view({
  id: "chat-agent",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const toggle = (api, event) =>
      api.act("toggle", { index: d.index, open: !d.open, keyboard: keyed(event) });
    const head = ui.row(
      {
        id: "tool-" + d.index,
        key: "call",
        h: m.control,
        pl: 10,
        pr: 6,
        items: "center",
        gap: 10,
        border: "1px",
        borderColor: "transparent",
        radius: m.radiusMedium - 1,
        cursor: "pointer",
        hover: { bg: "hover" },
        ring: { borderColor: "accent" },
        onClick: toggle,
      },
      [
        ui.monogram(d.code, { size: m.slot - 2 }),
        ui.text(d.title, { minW: 0, shrink: 1, truncate: true, size: 13, color: "fg" }),
        // Its kind gives way first, then its counts, then what it was for.
        d.kind ? meta(d.kind, { minW: 0, truncate: true, shrink: 1000 }) : null,
        ui.box({ grow: 1 }),
        d.about ? meta(d.about, { minW: 0, truncate: true, shrink: 500 }) : null,
        ui.row({ font: "mono", size: 11, color: "fgMuted", shrink: 0, items: "center", gap: 8 }, [
          slot(d, "took"),
          ui.text(d.word.toUpperCase(), { id: "agent-state-" + d.index, weight: "bold", size: 10, color: d.tone }),
        ]),
        chevron(d.open),
      ],
    );
    return ui.box(
      {
        position: "relative",
        w: "full",
        radius: m.radiusMedium,
        border: "1px",
        borderColor: BORDERS[d.border],
        bg: "raised",
        overflow: "hidden",
      },
      [
        head,
        has(d, "latest") ? latestLine("latest", { id: "agent-latest-" + d.index }) : null,
        slot(d, "body"),
        d.asks ? asks(d.asks, m) : d.answers ? answers(d.answers, m) : null,
        slot(d, "spot"),
      ],
    );
  },
});

// The card of an agent started in a pane: its
// harness's tile (`code`), its title, its time (the app's, `time`: it counts
// while the agent runs) and how it goes (`word` in `tone`); under them its
// latest words (the app's, `detail`). A click opens the agent.
kou.view({
  id: "chat-nested",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    return ui.box(
      {
        id: "nested-" + d.id,
        key: "nested",
        position: "relative",
        w: "full",
        radius: m.radiusMedium,
        border: "1px",
        borderColor: d.running ? BORDERS.working : "line",
        bg: "raised",
        cursor: "pointer",
        hover: { bg: "hover" },
        ring: { borderColor: "accent" },
        onClick: (api, event) => api.act("nested", { id: d.id, keyboard: keyed(event) }),
      },
      [
        ui.row({ h: m.control, pl: 10, pr: 6, items: "center", gap: 10 }, [
          ui.monogram(d.code, { size: m.slot - 2 }),
          ui.text(d.title, { minW: 0, grow: 1, truncate: true, size: 13, color: "fg" }),
          ui.box({ shrink: 0, font: "mono", size: 11, color: "fgMuted" }, [slot(d, "time")]),
          ui.text(d.word.toUpperCase(), { shrink: 0, font: "mono", weight: "bold", size: 10, color: d.tone }),
          chevron(false),
        ]),
        latestLine("detail"),
        slot(d, "counting"),
        slot(d, "spot"),
      ],
    );
  },
});

// The card beside the transcript, at the pane's top right: the project
// (`project`) with buttons that open its folder and put the card away; the
// branch (`branch`) and its changes (`changes`: lines added and removed);
// who started whom (`lineage`: `running`, `agents`, `earlier`, the number
// that went before); and how full the context is (`context`: `used`,
// `limit`, `figures`). `lifted`: it lies over the text, with a shadow.
// Where it stands, how it slides in, its height measured (`measure`) and
// the times that count (`counting`) are the app's.
//
// Its sizes are the quieter look's picture at that look's type size: a row
// 30 high (the picture's 28, kept whole), in from the card's edge 16.5 and
// from its end 17.6, its words from 40.6.
const BESIDE_ROW = 30;
const BESIDE_PAD = 7;
const BESIDE_GAP = 40.6 - 16.5 - 18;
const besideRow = (props, children) =>
  ui.row({ h: BESIDE_ROW, mx: 8, pl: 16.5 - 8, pr: 17.6 - 8, items: "center", gap: BESIDE_GAP, ...props }, children);
// The box a row's picture sits in.
const besidePicture = (picture) => ui.row({ shrink: 0, w: 18, h: 18, items: "center", justify: "center" }, [picture]);
const besideWords = (text) => ui.text(text, { minW: 0, grow: 1, truncate: true, color: "fgDim" });

kou.view({
  id: "chat-beside",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const button = (id, icon, tooltip, onClick) =>
      ui.row(
        {
          id,
          key: id,
          shrink: 0,
          w: 22,
          h: 22,
          mr: -4,
          radius: m.radius,
          border: "1px",
          borderColor: "transparent",
          items: "center",
          justify: "center",
          cursor: "pointer",
          hover: { bg: "selected" },
          ring: { borderColor: "accent" },
          tooltip,
          onClick,
        },
        [ui.icon(icon, { size: 14, color: "fgMuted" })],
      );
    const row = (icon, text, end) =>
      besideRow({}, [besidePicture(ui.icon(icon, { size: 16, color: "fgDim" })), besideWords(text), end]);
    // An agent's picture with its state's dot, ringed in the card's colour.
    const picture = (agent) => {
      const at = agent.parent ? 5 : -5;
      const shapes = {
        lineage: () => ui.icon("lineage", { size: 14, color: "fgMuted" }),
        terminal: () => ui.icon("terminal", { size: 14, color: "fgMuted" }),
        brand: () => ui.brand(agent.harness, { code: agent.code, size: 14 }),
        letters: () =>
          ui.text(agent.letters, {
            w: 14,
            h: 14,
            dir: "row",
            items: "center",
            justify: "center",
            font: "mono",
            weight: "bold",
            // 14 × 0.55 as the app's own works it out, in single precision.
            size: 7.7000003,
            color: "fgDim",
          }),
      };
      return ui.row({ position: "relative", items: "center", justify: "center" }, [
        shapes[agent.mark](),
        ui.box({
          position: "absolute",
          left: 7 + at - 3.5,
          top: 7 + at - 3.5,
          w: 7,
          h: 7,
          radius: "full",
          border: "1px",
          borderColor: "raised",
          bg: agent.dot,
        }),
      ]);
    };
    // One the agent started by itself shows its card, open; one in a pane
    // is opened.
    const agentRow = (agent) =>
      besideRow(
        {
          id: "lineage-" + agent.id,
          key: "lineage-" + agent.id,
          radius: m.radius,
          border: "1px",
          borderColor: "transparent",
          cursor: "pointer",
          hover: { bg: "hover" },
          ring: { borderColor: "accent" },
          onClick: (api, event) =>
            agent.call === null || agent.call === undefined
              ? api.act("nested", { id: agent.id, keyboard: keyed(event) })
              : api.act("show", { index: agent.call, keyboard: keyed(event) }),
        },
        [
          besidePicture(picture(agent)),
          besideWords(agent.title),
          ui.text(agent.state, { shrink: 0, size: 9.8, color: agent.tone }),
        ],
      );
    const sections = [
      [
        row(
          "project",
          d.project,
          ui.row({ shrink: 0, items: "center" }, [
            button("beside-folder", "folder", "Open the folder", (api) => api.act("folder")),
            button("beside-hide", "chevron-right", "Put this card away", (api) => api.act("beside", { open: false })),
          ]),
        ),
      ],
      [
        d.branch ? row("branch", d.branch) : null,
        d.changes
          ? row(
              "diff",
              "Changes",
              ui.row({ shrink: 0, gap: 6, font: "mono", size: 11.5 }, [
                ui.text("+" + d.changes[0], { color: d.changes[0] > 0 ? "ok" : "fgMuted" }),
                ui.text("-" + d.changes[1], { color: d.changes[1] > 0 ? "error" : "fgMuted" }),
              ]),
            )
          : null,
      ].filter(Boolean),
      d.lineage
        ? [
            ui.text(d.running > 0 ? "LINEAGE · " + d.running + " RUNNING" : "LINEAGE", {
              h: BESIDE_ROW + 4,
              px: 16.5,
              dir: "row",
              items: "center",
              font: "mono",
              weight: "bold",
              size: 10.5,
              color: "fgMuted",
            }),
            d.agents.map(agentRow),
            d.earlier > 0
              ? row("agents", "Previous agents", ui.text(String(d.earlier), { shrink: 0, color: "fgMuted" }))
              : null,
          ]
        : [],
    ].filter((rows) => rows.length > 0);
    const parts = sections.map((rows, at) =>
      ui.col({ w: "full", py: BESIDE_PAD, ...(at > 0 ? { borderT: "1px", borderColor: "line" } : {}) }, rows),
    );
    const c = d.context;
    if (c) {
      // A track filled as far as the context is, a mark where it ends.
      const share = Math.min(1, Math.max(0, c.used / c.limit));
      const fill = share >= 0.9 ? "error" : share >= 0.75 ? "blocked" : "accent";
      parts.push(
        ui.box({ w: "full", ...(parts.length > 0 ? { borderT: "1px", borderColor: "line" } : {}) }, [
          ui.row({ id: "beside-context", h: 37, pl: 16.5, pr: 17.6 - 4, items: "center", gap: 10 }, [
            label("Context", "fgMuted"),
            ui.row({ grow: 1, items: "center" }, [
              ui.box({ position: "relative", w: 100, maxW: "full", h: 2, radius: "full", bg: "lineStrong" }, [
                ui.box({ position: "absolute", top: 0, left: 0, h: "full", w: 100 * share, radius: "full", bg: fill }),
                ui.box({ position: "absolute", top: -3, left: 100 * share - 0.5, w: 1, h: 8, bg: fill }),
              ]),
            ]),
            ui.text(c.figures, { shrink: 0, color: "fgDim" }),
          ]),
        ]),
      );
    }
    return ui.col(
      {
        id: "beside",
        occlude: true,
        w: (244 * 13) / 12,
        radius: m.radiusLarge,
        border: "1px",
        borderColor: "lineStrong",
        bg: "raised",
        ...(d.lifted ? { shadow: "float" } : {}),
        overflow: "hidden",
        size: m.text,
      },
      [parts, slot(d, "counting"), slot(d, "measure")],
    );
  },
});

// The handle at the pane's right edge where the card beside would stand,
// put away: it pulls the card out.
kou.view({
  id: "chat-beside-handle",
  render(ctx) {
    return ui.row(
      {
        id: "beside-pull",
        key: "beside-pull",
        occlude: true,
        w: 20,
        h: BESIDE_ROW + 2 * BESIDE_PAD,
        radiusL: ctx.theme.metric.radius,
        border: "1px",
        borderColor: "lineStrong",
        bg: "raised",
        items: "center",
        justify: "center",
        cursor: "pointer",
        hover: { bg: "selected" },
        ring: { borderColor: "accent" },
        tooltip: "Show the project card",
        onClick: (api) => api.act("beside", { open: true }),
      },
      [ui.icon("chevron-right", { size: 12, color: "fgMuted", rotate: 180 })],
    );
  },
});

// The ticks down the transcript's left edge, one for each prompt (`ticks`:
// `{ index, said, on }`, `on` while its prompt is on screen): the pointer
// on one says what it asked, and a click goes to it.
kou.view({
  id: "chat-ticks",
  render(ctx) {
    return ui.col(
      { id: "ticks", position: "absolute", top: 0, bottom: 0, left: 0, justify: "center" },
      ctx.data.ticks.map((tick) => {
        const group = "tick-" + tick.index;
        return ui.row(
          {
            id: group,
            key: group,
            group,
            w: 12,
            h: 7,
            justify: "center",
            items: "center",
            cursor: "pointer",
            tooltip: tick.said,
            onClick: (api) => api.act("tick", { index: tick.index }),
          },
          [
            ui.box({
              w: 5,
              h: 1.5,
              radius: "full",
              bg: tick.on ? "fgFaint" : "alpha(fgFaint, 0.5)",
              groupHover: { group, bg: "fgDim" },
            }),
          ],
        );
      }),
    );
  },
});

// A plan (`.plan`): how far it has come, and its steps. The mark of the
// step that runs blinks: it is the app's (`mark`).
kou.view({
  id: "chat-plan",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const done = d.steps.filter((step) => step.status === "done").length;
    const mark = {
      pending: () => ui.box({ w: 8, h: 8, radius: m.radiusMark, border: "1px", borderColor: "fgFaint" }),
      running: () => slot(d, "mark"),
      done: () => ui.icon("check", { size: 12, color: "ok" }),
    };
    const ink = { pending: "fgDim", running: "fg", done: "fgMuted" };
    return ui.box({ w: "full" }, [
      metaLine([label("Plan", "fgDim"), meta(done + " of " + d.steps.length + " done")]),
      ui.col(
        { mt: 4, gap: 2 },
        d.steps.map((step) =>
          ui.row({ items: "start", gap: 10 }, [
            ui.row({ shrink: 0, w: 14, h: 22, items: "center", justify: "center" }, [mark[step.status]()]),
            ui.text(step.text, {
              minW: 0,
              size: 13.5,
              lineHeight: 22,
              color: ink[step.status],
              strike: step.status === "done",
            }),
          ]),
        ),
      ),
    ]);
  },
});

// A message from the backend: a line in capitals with a rule after it, or
// a warning or an error in a box of its tone (`turn`: the error a turn
// ended with).
kou.view({
  id: "chat-notice",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    if (d.level === "info") {
      return ui.row({ minH: META, items: "center", gap: 12 }, [
        ui.text(d.text.toUpperCase(), {
          minW: 0,
          font: "mono",
          weight: "bold",
          size: 10.5,
          lineHeight: 16,
          color: "fgMuted",
        }),
        ui.box({ grow: 1, minW: 16, h: 1, bg: "line" }),
      ]);
    }
    const tone = d.level === "warning" ? "blocked" : "error";
    const word = d.level === "warning" ? "Warning" : d.turn ? "Turn failed" : "Error";
    return ui.box({ w: "full" }, [
      metaLine([label(word, tone)]),
      ui.text(d.text, {
        mt: 6,
        w: "full",
        px: 13,
        py: 10,
        radius: m.radiusMedium,
        border: "1px",
        borderColor: "mix(line, " + tone + ", 0.35)",
        bg: "alpha(" + tone + ", " + (ctx.theme.dark ? 0.09 : 0.07) + ")",
        font: "mono",
        size: 12.5,
        lineHeight: 19,
        color: "fg",
      }),
    ]);
  },
});

// The line a run of work folds into once it is over (`words`: how long it
// took or how many steps it was); a click opens the run under it.
kou.view({
  id: "chat-worked",
  render(ctx) {
    const d = ctx.data;
    return ui.row({}, [
      ui.row(
        {
          id: "worked-" + d.start,
          key: "worked",
          h: META,
          mx: -6,
          px: 6,
          items: "center",
          gap: 8,
          radius: ctx.theme.metric.radius,
          border: "1px",
          borderColor: "transparent",
          cursor: "pointer",
          hover: { bg: "hover" },
          ring: { borderColor: "accent" },
          onClick: (api, event) => api.act("worked", { start: d.start, keyboard: keyed(event) }),
        },
        [label(d.words, "fgMuted"), chevron(d.open)],
      ),
    ]);
  },
});

// The web cockpit's `.act`: a word in capitals of the monospace, framed in
// `borderColor`, filled under the pointer.
const act = (id, text, m, onClick, borderColor) =>
  ui.text(text.toUpperCase(), {
    id,
    key: id,
    shrink: 0,
    h: 22,
    px: 7,
    radius: m.radius,
    border: "1px",
    borderColor: borderColor || "transparent",
    dir: "row",
    items: "center",
    cursor: "pointer",
    font: "mono",
    weight: "semibold",
    size: 10,
    color: "fgMuted",
    hover: { borderColor: "lineStrong", bg: "pane", color: "fg" },
    ring: { borderColor: "accent", color: "fg" },
    onClick,
  });

// What a turn or a session ended with, in a box of its tone.
const errorBox = (text, tone, theme) =>
  ui.text(text, {
    mt: 6,
    w: "full",
    px: 13,
    py: 10,
    radius: theme.metric.radiusMedium,
    border: "1px",
    borderColor: "mix(line, " + tone + ", 0.35)",
    bg: "alpha(" + tone + ", " + (theme.dark ? 0.09 : 0.07) + ")",
    font: "mono",
    size: 12.5,
    lineHeight: 19,
    color: "fg",
  });

// The line after the last entry: what the chat is waiting
// for (`text`), or what ended the session (`agent`, `reason`).
kou.view({
  id: "chat-tail",
  render(ctx) {
    const d = ctx.data;
    if (d.reason !== undefined && d.reason !== null) {
      return ui.col({ id: "tail" }, [
        metaLine([label(d.agent + " stopped", "error")]),
        errorBox(d.reason, "error", ctx.theme),
        ui.text(
          "Close this pane and open it again to start a new session. " +
            "The conversation above stays readable until then.",
          { mt: 8, color: "fgMuted" },
        ),
      ]);
    }
    return ui.row({ id: "tail", minH: META, items: "center", color: "fgMuted" }, [d.text]);
  },
});

// What `/clear` left: how much is out of sight
// (`hidden`), and a way back to it, which asks `unclear`.
kou.view({
  id: "chat-cleared",
  render(ctx) {
    const d = ctx.data;
    const said = d.hidden + " EARLIER " + (d.hidden === 1 ? "ENTRY IS" : "ENTRIES ARE") + " OUT OF SIGHT";
    return ui.row(
      { minH: META, items: "center", gap: 12, font: "mono", weight: "bold", size: 10.5, color: "fgMuted" },
      [
        said,
        act("unclear", "Show them", ctx.theme.metric, (api) => api.act("unclear"), "lineStrong"),
        ui.box({ grow: 1, h: 1, bg: "line" }),
      ],
    );
  },
});

// A key to press and what it does, in a list's foot.
const hint = (keys, what, m) => ui.row({ shrink: 0, items: "center", gap: 4 }, [key(keys, m), what]);

// A line of the list over the field (`.commands`): a file with its folder,
// a command with its other name and what it takes, a value of a command's
// (`current`: the one in force), a hint, a prompt of the history; `on`, the
// keys are on it.
function menuLine(line, m) {
  const mono = (text, props) => ui.text(text, { shrink: 0, font: "mono", size: 12.5, ...props });
  // What it does, quieter, brighter on the line chosen.
  const detail = () =>
    ui.text(line.detail, { minW: 0, grow: 1, truncate: true, size: 12.5, color: line.on ? "fgDim" : "fgMuted" });
  const small = (text) => ui.text(text, { shrink: 0, size: m.textSmall, color: "fgMuted" });
  let inner;
  if (line.kind === "file") {
    inner = [
      ui.icon(line.directory ? "folder" : "file", { size: 13, color: line.on ? "accent" : "fgMuted" }),
      ui.row({ minW: 0, grow: 1, items: "center", font: "mono", size: m.text - 0.5 }, [
        ui.text(line.dir, { minW: 0, truncate: "start", color: "fgMuted" }),
        ui.text(line.name, { shrink: 0, weight: "semibold", color: line.on ? "accent" : "fg" }),
      ]),
      line.detail ? small(line.detail) : null,
    ];
  } else if (line.kind === "command") {
    inner = [
      ui.row({ shrink: 0, items: "baseline", gap: 5 }, [
        mono(line.name, { weight: "semibold", color: line.on ? "accent" : "fg" }),
        line.alias ? mono(line.alias, { color: "fgMuted" }) : null,
        line.args ? mono(line.args, { color: "fgMuted" }) : null,
      ]),
      detail(),
    ];
  } else if (line.kind === "value") {
    inner = [
      mono(line.name, { weight: "semibold", color: line.on ? "accent" : "fg" }),
      detail(),
      line.current ? small("current") : null,
    ];
  } else if (line.kind === "prompt") {
    inner = [
      ui.text(line.name, { minW: 0, grow: 1, truncate: true, color: line.on ? "fg" : "fgDim" }),
      small(line.detail),
    ];
  } else {
    inner = [mono(line.name, { color: "fgDim" }), detail()];
  }
  return ui.row({ position: "relative", h: 28, px: 14, items: "center", gap: 14 }, inner);
}

// The list a `/`, a `$` or Ctrl+R opens over the field: its card, its lines
// and its foot. The highlight under the line the keys are on (`highlight`)
// and where each line takes the pointer (`hits`) are the field's own.
// `ctx.data`: `height` (of the lines), `loading` (the files are still being
// listed), `lines` (see `menuLine`), and `footer`: `{ take, enter, run,
// note }`, what Tab (Enter, with `enter`) does, whether Enter runs, and a
// note at its end.
kou.view({
  id: "chat-menu",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const list = ui.box({ position: "relative", shrink: 0, h: d.height, py: 4 }, [
      slot(d, "highlight"),
      d.loading
        ? ui.row({ h: 28, px: 14, items: "center", font: "mono", size: 11.5, color: "fgMuted" }, [
            "Listing the project's files",
          ])
        : null,
      d.lines.map((line) => menuLine(line, m)),
      slot(d, "hits"),
    ]);
    const f = d.footer;
    const footer = f
      ? ui.row(
          {
            shrink: 0,
            h: 28,
            px: 14,
            items: "center",
            gap: 16,
            borderT: "1px",
            borderColor: "line",
            font: "mono",
            size: 10.5,
            color: "fgMuted",
          },
          [
            hint("↑↓", "choose", m),
            hint(f.enter ? "↵" : "tab", f.take, m),
            f.run ? hint("↵", "run", m) : null,
            hint("esc", "close", m),
            ui.text(f.note, { minW: 0, grow: 1, truncate: true, align: "right", font: "mono" }),
          ],
        )
      : null;
    return ui.col(
      {
        id: "commands-card",
        shadow: "float",
        w: "full",
        radius: m.radiusLarge,
        border: "1px",
        borderColor: "lineStrong",
        bg: "raised",
        overflow: "hidden",
        justify: "end",
        size: 13,
        color: "fg",
      },
      [list, footer],
    );
  },
});

// The help card over the field: the keys and what each does (`keys`, pairs
// of a key and what it does), and the commands with what each takes and
// does (`commands`, pairs). Where it stands and how it comes in are the
// field's; it scrolls in itself.
kou.view({
  id: "chat-help",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const head = (text, props) => ui.text(text, { weight: "medium", color: "fg", ...props });
    return ui.col(
      {
        id: "help-card",
        shadow: "float",
        key: "help",
        w: "full",
        maxH: 440,
        overflow: "scroll",
        p: 12,
        radius: m.radiusLarge,
        border: "1px",
        borderColor: "lineStrong",
        bg: "raised",
        gap: 6,
        size: m.textSmall,
      },
      [
        head("Keys"),
        d.keys.map(([keys, what]) =>
          ui.row({ items: "start", gap: 10 }, [
            ui.row({ shrink: 0, w: 84 }, [key(keys, m)]),
            ui.text(what, { minW: 0, grow: 1, color: "fgDim" }),
          ]),
        ),
        head("Commands", { pt: 6 }),
        d.commands.map(([name, help]) =>
          ui.row({ items: "start", gap: 10 }, [
            ui.text(name, { shrink: 0, w: 180, font: "mono", color: "fg" }),
            ui.text(help, { minW: 0, grow: 1, color: "fgDim" }),
          ]),
        ),
      ],
    );
  },
});

// The list of a picker over its control (the modes'): its card, which
// scrolls, and what it is for (`title`) over its lines, which are the
// field's own (`choices`): the keys move through them and the pointer
// lights them. A list's rows, as a menu has them: no taller than the rail's.
kou.view({
  id: "chat-choices",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    return ui.box(
      {
        id: "choices-card",
        shadow: "float",
        key: "choices",
        w: 224,
        maxH: 320,
        overflow: "scroll",
        py: 4,
        radius: m.radiusLarge,
        border: "1px",
        borderColor: "lineStrong",
        bg: "raised",
        size: 13,
        color: "fg",
      },
      [ui.box({ px: 14, pt: 6, pb: 4 }, [label(d.title, "fgMuted")]), slot(d, "choices")],
    );
  },
});

// What a line of that list says: the choice's `name`, a check when it is
// the one in force (`current`), and what it does (`description`).
kou.view({
  id: "chat-choice",
  render(ctx) {
    const d = ctx.data;
    return ui.box({}, [
      ui.row({ h: 18, items: "center", gap: 8 }, [
        ui.text(d.name, { minW: 0, truncate: true, color: d.current ? "accent" : "fg" }),
        ui.box({ grow: 1 }),
        d.current ? ui.icon("check", { size: 14, color: "accent" }) : null,
      ]),
      d.description
        ? ui.text(d.description, { font: "mono", size: 10.5, lineHeight: 15, color: "fgMuted" })
        : null,
    ]);
  },
});

// The model picker over the model's control: its card and its foot (how
// many models and providers, `count`, and its keys, `keys`: pairs of a key
// and what it does). The search field, which takes what is typed
// (`search`), and the list, which scrolls and slides between the providers
// and their models (`list`), are the field's own.
kou.view({
  id: "chat-models",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    return ui.col(
      {
        id: "models-card",
        shadow: "float",
        w: "full",
        radius: m.radiusLarge,
        border: "1px",
        borderColor: "lineStrong",
        bg: "raised",
        overflow: "hidden",
        color: "fg",
      },
      [
        slot(d, "search"),
        slot(d, "list"),
        ui.row(
          {
            h: 28,
            pl: 14,
            pr: 10,
            items: "center",
            gap: 10,
            borderT: "1px",
            borderColor: "line",
            font: "mono",
            size: 10.5,
            color: "fgMuted",
          },
          [
            ui.text(d.count, { minW: 0, grow: 1, truncate: true }),
            ui.row(
              { shrink: 0, items: "center", gap: 4 },
              d.keys.map(([keys, word]) => ui.row({ items: "center", gap: 3 }, [key(keys, m), word || null])),
            ),
          ],
        ),
      ],
    );
  },
});

// What a line of the model picker says, by its `kind`: a provider, the row
// back to the providers (`back`, which asks `back`), a provider's heading
// over what a search found, a model, a model ID typed in (`free`), or
// nothing found (`nothing`). A provider's colour is `provider(id)`. The
// shape of a line the keys move through, and its highlight, are the
// field's: a provider's, a model's and a typed ID's lines are drawn in it.
kou.view({
  id: "chat-model-line",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const hue = (id) => "provider(" + id + ")";
    const dot = (id, side) => ui.box({ shrink: 0, w: side, h: side, radius: side / 2, bg: hue(id) });
    // A chip that flags a line: media.
    const flag = (text) =>
      ui.text(text.toUpperCase(), {
        shrink: 0,
        px: 5,
        h: 16,
        dir: "row",
        items: "center",
        radius: m.radiusSmall,
        border: "1px",
        borderColor: "lineStrong",
        font: "mono",
        size: 10,
        color: "fgMuted",
      });
    // The mark of the model in use, and of the provider that has it.
    const current = () => ui.icon("check", { size: 14, color: "accent" });
    const inLine = (children) => ui.row({ grow: 1, minW: 0, items: "center", gap: 8 }, children);
    if (d.kind === "provider") {
      return inLine([
        dot(d.id, 8),
        // Its name, and its newest models after it on the same line.
        ui.row({ minW: 0, grow: 1, items: "baseline", gap: 8 }, [
          ui.text(d.name, {
            shrink: 0,
            maxW: 160,
            truncate: true,
            weight: "semibold",
            size: 13,
            color: d.selected ? "mix(fg, " + hue(d.id) + ", 0.72)" : "fg",
          }),
          ui.text(d.newest, {
            minW: 0,
            grow: 1,
            truncate: true,
            font: "mono",
            size: 11,
            color: d.selected ? "fgDim" : "fgMuted",
          }),
        ]),
        d.current ? current() : null,
        ui.text(String(d.count), {
          shrink: 0,
          minW: 18,
          dir: "row",
          justify: "end",
          font: "mono",
          weight: "bold",
          size: 11.5,
          color: "fgMuted",
        }),
        ui.row({ shrink: 0, w: 14, justify: "center" }, [
          ui.icon("chevron-right", { size: 12, color: d.selected ? "accent" : "fgFaint" }),
        ]),
      ]);
    }
    if (d.kind === "model" || d.kind === "free") {
      const words =
        d.kind === "model"
          ? [
              ui.text(d.name, {
                shrink: 0,
                maxW: 260,
                truncate: true,
                weight: "semibold",
                size: 13,
                color: d.current ? "accent" : "fg",
              }),
              d.id ? ui.text(d.id, { minW: 0, truncate: true, font: "mono", size: 11.5, color: "fgMuted" }) : null,
            ]
          : [
              ui.text("Use “" + d.id + "”", { shrink: 0, font: "mono", size: 12.5, color: "fg" }),
              ui.text("not listed", { font: "mono", size: 11.5, color: "fgMuted" }),
            ];
      return inLine([
        dot(d.kind === "model" ? d.provider : "", 7),
        ui.row({ minW: 0, grow: 1, items: "baseline", gap: 8 }, words),
        d.media ? flag("media") : null,
        d.current ? current() : null,
      ]);
    }
    if (d.kind === "back") {
      return ui.row(
        { h: 32, pl: 8, pr: 14, items: "center", gap: 8, borderB: "1px", borderColor: "line", mb: 4 },
        [
          ui.row(
            {
              id: "model-back",
              key: d.live ? "model-back" : "model-back-old",
              shrink: 0,
              h: 22,
              pl: 5,
              pr: 8,
              items: "center",
              gap: 4,
              radius: m.radius,
              border: "1px",
              borderColor: "lineStrong",
              font: "mono",
              weight: "bold",
              size: 10,
              color: "fgMuted",
              cursor: "pointer",
              hover: { borderColor: "fgFaint", color: "fg" },
              ...(d.live ? { onClick: (api) => api.act("back") } : {}),
            },
            [ui.icon("chevron-right", { size: 12, color: "fgMuted", rotate: 180 }), "PROVIDERS"],
          ),
          dot(d.id, 8),
          ui.text(d.name, {
            minW: 0,
            grow: 1,
            truncate: true,
            weight: "semibold",
            size: 13,
            color: "mix(fg, " + hue(d.id) + ", 0.72)",
          }),
          ui.text(d.count + (d.count === 1 ? " model" : " models"), { shrink: 0, font: "mono", size: 11, color: "fgMuted" }),
        ],
      );
    }
    if (d.kind === "heading") {
      return ui.row(
        {
          h: 24,
          px: 14,
          pt: 4,
          items: "center",
          gap: 8,
          font: "mono",
          weight: "bold",
          size: 10,
          color: "mix(fgMuted, " + hue(d.id) + ", 0.7)",
        },
        [d.name.toUpperCase(), ui.text(String(d.hits), { weight: "normal", color: "fgMuted" })],
      );
    }
    return ui.row({ h: 30, px: 14, items: "center", size: 12.5, color: "fgMuted" }, ["No model matches."]);
  },
});

// The card of a surface the agent made (its `UI` tool): its head (the
// surface's `title`, what stands beside it, `tag`, and how it stands,
// `status`, `live` while it follows its sources), which opens and closes
// it; what it shows is the app's own (`body`).
kou.view({
  id: "chat-app-card",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const toggle = (api, event) =>
      api.act("toggle", { index: d.index, open: !d.open, keyboard: keyed(event) });
    return ui.box(
      { position: "relative", w: "full", radius: m.radiusMedium, border: "1px", borderColor: "line", bg: "raised", overflow: "hidden" },
      [
        ui.row(
          {
            id: "tool-" + d.index,
            key: "call",
            h: m.control,
            pl: 10,
            pr: 6,
            items: "center",
            gap: 10,
            border: "1px",
            borderColor: "transparent",
            radius: m.radiusMedium - 1,
            cursor: "pointer",
            hover: { bg: "hover" },
            ring: { borderColor: "accent" },
            onClick: toggle,
          },
          [
            ui.row({ shrink: 0, w: 14, justify: "center" }, [
              ui.icon("canvas", { size: 13, color: d.live ? "accent" : "fgMuted" }),
            ]),
            ui.text(d.title, { minW: 0, shrink: 1, truncate: true, size: 13, color: "fg" }),
            meta(d.tag, { minW: 0, truncate: true, shrink: 1000 }),
            ui.box({ grow: 1 }),
            ui.text(d.status.toUpperCase(), {
              id: "surface-status-" + d.index,
              shrink: 0,
              font: "mono",
              weight: "bold",
              size: 10,
              color: d.live ? "ok" : "fgMuted",
            }),
            chevron(d.open),
          ],
        ),
        slot(d, "body"),
      ],
    );
  },
});
