// The Usage page. What it shows comes from the app as `ctx.data` (see
// `Shell::usage_data` in kou-desktop/src/usage.rs); how it looks is all
// here, and a change to this file shows in the open window as it is saved.

// How tall an account's bar is, the narrowest it is drawn beside another
// before it goes under it, how wide the page's column gets, and the column
// a limit's name and share stand in, left of its bars.
const BAR = 32;
const BAR_MIN = 208;
const PAGE = 920;
const SAID = 172;

const percent = (share) => Math.round(share) + "%";
const of = (share) => (Math.max(0, Math.min(1, share)) * 100).toFixed(3) + "%";

// The colours of a share that is left: the accent while there is plenty,
// amber from a quarter down, red from a tenth. The first tints a bar, the
// second is the ink of the share where it is written large.
function tone(left) {
  if (left <= 10) return ["error", "error"];
  if (left <= 25) return ["blocked", "blocked"];
  return ["accent", "fg"];
}

// An account's bar for a limit: as much of it filled as the account has
// left, the rest hatched; its name and that share written in it; a line
// where the limit's time stands; and at its end, when it starts over.
function bar(number, part, fill, m) {
  const [tint] = tone(part.left);
  let said = part.account + ": " + percent(part.left) + " left";
  if (part.resets) said += ", starts over in " + part.resets;
  if (part.time !== null) {
    said += ". The line is the window's time: " + percent(part.time * 100) + " of it is left";
  }
  return ui.box(
    {
      id: "usage-bar-" + number,
      position: "relative",
      h: BAR,
      grow: 1,
      minW: BAR_MIN,
      radius: m.radius,
      overflow: "hidden",
      bg: "hover",
      tooltip: said,
    },
    [
      // What is used: hatched, under what is left.
      ui.box({ position: "absolute", inset: 0, bg: { hatch: ui.mix("hover", tint, 0.4), width: 1.5, gap: 5 } }),
      ui.box({
        id: "usage-fill-" + number,
        position: "absolute",
        top: 0,
        bottom: 0,
        left: 0,
        w: of((part.left / 100) * fill),
        radiusR: m.radius,
        bg: ui.mix("pane", tint, 0.34),
      }),
      // Cut clear of the fill on both sides, so that it reads over the fill
      // and over the hatching alike.
      part.time !== null &&
        ui.row(
          {
            position: "absolute",
            top: 0,
            bottom: 0,
            left: of(Math.max(0.01, Math.min(0.99, part.time))),
            ml: -3,
            w: 6,
            justify: "center",
            bg: "raised",
          },
          [ui.box({ w: 2, h: "full", bg: "fg" })],
        ),
      ui.row({ position: "absolute", inset: 0, pl: 12, pr: 5, items: "center", gap: 8 }, [
        ui.row({ grow: 1, minW: 0, items: "baseline", gap: 7, color: "fg" }, [
          ui.text(part.account, { minW: 0, truncate: true, weight: "medium" }),
          ui.text(percent(part.left), { shrink: 0 }),
        ]),
        part.resets &&
          ui.row(
            {
              shrink: 0,
              h: m.controlSmall,
              px: 7,
              items: "center",
              gap: 5,
              radius: m.radiusSmall,
              // (Opaque: the time's line passes behind it.)
              bg: "pane",
              size: m.textSmall,
              color: "fgDim",
            },
            [ui.icon("clock", { size: 12, color: "fgMuted" }), part.resets],
          ),
      ]),
    ],
  );
}

// A limit of a plan, as a card: at the left its name, what is left of it
// over all the accounts, and what the first reset gives back; at the right
// a bar for each account, side by side while they fit.
function meter(first, limit, fill, m) {
  const [, ink] = tone(limit.left);
  return ui.row(
    {
      w: "full",
      px: 16,
      py: 14,
      radius: m.radiusLarge,
      border: 1,
      borderColor: "line",
      bg: "raised",
      wrap: true,
      items: "center",
      gapX: 24,
      gapY: 12,
    },
    [
      ui.col({ w: SAID, shrink: 0 }, [
        ui.text(limit.label, { color: "fgDim" }),
        ui.row({ items: "baseline", gap: 6 }, [
          ui.text(percent(limit.left), { size: 28, lineHeight: 36, weight: "semibold", color: ink }),
          ui.text("left", { color: "fgMuted" }),
        ]),
        ui.text(limit.back || "", { h: 16, size: m.textSmall, color: "fgMuted" }),
      ]),
      ui.row(
        { grow: 1, minW: BAR_MIN, wrap: true, gap: 8 },
        limit.parts.map((part, at) => bar(first + at, part, fill, m)),
      ),
    ],
  );
}

// A chat's row: its state, its name, how full its context is, and what it
// has spent. A click goes to its pane.
function chat(number, row, m) {
  const full = row.context;
  const [tint, ink] = full === null ? ["fgDim", "fgDim"] : tone(100 - full);
  return ui.row(
    {
      id: "usage-chat-" + number,
      h: m.row,
      mx: -6,
      px: 6,
      items: "center",
      gap: 8,
      radius: m.radius,
      cursor: "pointer",
      focusable: true,
      hover: { bg: "hover" },
      tooltip: row.said,
      onClick: (api) => api.act("pane.focus", { pane: row.pane }),
    },
    [
      ui.mark(row.mark),
      ui.text(row.title, { grow: 1, minW: 0, truncate: true, color: "fg" }),
      row.account && ui.text(row.account, { shrink: 0, size: m.textSmall, color: "fgMuted" }),
      full !== null &&
        ui.row({ shrink: 0, items: "center", gap: 6, size: m.textSmall }, [
          ui.box({ w: 44, h: 4, radius: "full", bg: "lineStrong", overflow: "hidden" }, [
            ui.box({ h: "full", w: of(full / 100), radius: "full", bg: tint === "accent" ? "fgDim" : tint }),
          ]),
          ui.row({ w: 30, justify: "end", color: ink === "fg" ? "fgDim" : ink }, [percent(full)]),
        ]),
      row.cost && ui.row({ w: 46, shrink: 0, justify: "end", size: m.textSmall, color: "fgMuted" }, [row.cost]),
    ],
  );
}

// A provider's part of the page: who it is, a card for each limit of its
// plan, what its accounts without limits say, and its chats.
function provider(counts, block, fill, m) {
  const cards = [];
  for (const limit of block.limits) {
    cards.push(meter(counts.bars, limit, fill, m));
    counts.bars += limit.parts.length;
  }
  const notes = block.notes.map((note) =>
    ui.row({ items: "start", gap: 8 }, [
      ui.row({ shrink: 0, h: 16, items: "center" }, [ui.icon("info", { size: 12, color: "fgFaint" })]),
      ui.text(note, { grow: 1, minW: 0 }),
    ]),
  );
  const rows = block.chats.map((row) => chat(counts.chats++, row, m));
  return ui.col({ w: "full", gap: 8 }, [
    ui.row({ h: m.row, items: "center", gap: 10 }, [
      ui.monogram(block.code),
      ui.text(block.name, { shrink: 0, weight: "medium", color: "fg" }),
      ui.text(block.about, { grow: 1, minW: 0, truncate: true, size: m.textSmall, color: "fgMuted" }),
    ]),
    cards,
    notes.length > 0 && ui.col({ px: 2, gap: 6, size: m.textSmall, color: "fgMuted" }, notes),
    ui.col({ px: 2, gap: 2 }, [
      ui.row({ h: 20, items: "center", size: m.textSmall, color: "fgMuted" }, [block.spent]),
      rows,
    ]),
  ]);
}

kou.page({
  id: "usage",
  title: "Usage",
  icon: "flame",
  render(ctx) {
    const m = ctx.theme.metric;
    const data = ctx.data;
    const counts = { bars: 0, chats: 0 };
    const parts = data.providers.map((block) => provider(counts, block, data.fill, m));
    return ui.col({ maxW: PAGE, px: 32, pt: 24, pb: 40, gap: 28 }, [
      ui.row({ items: "center", gap: 10, mb: -12 }, [
        ui.text("Usage", { grow: 1, size: 15, weight: "medium", color: "fg" }),
        ui.text(data.updated, { shrink: 0, size: m.textSmall, color: "fgMuted" }),
        ui.button("Refresh", {
          id: "usage-refresh",
          small: true,
          tooltip: "Ask every account for its limits again",
          onClick: (api) => api.act("usage.refresh"),
        }),
      ]),
      parts.length > 0
        ? parts
        : ui.text("No agent that reports its plan's usage is installed", { py: 48, color: "fgMuted" }),
    ]);
  },
});
