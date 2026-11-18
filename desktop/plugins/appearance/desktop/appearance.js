// Rust owns validated theme documents and text editing. This view owns the
// library, editor layout, field frames and previews; slots contain text only.
const groups = [
  [
    "Surfaces",
    [
      ["pane", "Workspace"],
      ["rail", "Sidebar"],
      ["raised", "Cards & input"],
      ["code", "Code background"],
      ["hover", "Hover"],
      ["selected", "Selection & bubbles"],
      ["line", "Dividers"],
      ["line_strong", "Borders"],
      ["chip", "Inline code background"],
    ],
  ],
  [
    "Text",
    [
      ["fg", "Main text"],
      ["fg_dim", "Secondary text"],
      ["fg_muted", "Muted text"],
      ["fg_faint", "Disabled icons"],
      ["chip_ink", "Inline code text"],
    ],
  ],
  [
    "Actions",
    [
      ["accent", "Accent & links"],
      ["accent_ink", "Text on accent"],
      ["primary", "Send button"],
      ["primary_rim", "Button border"],
      ["primary_ink", "Button text"],
    ],
  ],
  [
    "Status",
    [
      ["working", "Agent working"],
      ["done", "Agent finished"],
      ["blocked", "Needs your answer"],
      ["error", "Errors"],
      ["ok", "Success"],
      ["diff_add", "Added lines"],
      ["diff_del", "Removed lines"],
    ],
  ],
];
const hex = (rgb) => "#" + rgb.toString(16).padStart(6, "0");
const palette = (doc) =>
  Object.fromEntries(Object.entries(doc.colors).map(([k, v]) => [k, hex(v)]));
const row = (children, props) =>
  ui.row({ items: "center", wrap: true, gap: 8, ...props }, children);
const caption = (text) => ui.text(text, { size: 12, color: "fgMuted" });
const heading = (text) => ui.text(text, { size: 15, weight: "semibold" });
const action = (id, label, name, given = {}, primary = false) =>
  ui.button(label, {
    id,
    key: id,
    primary,
    onClick: (api) => api.act(name, given),
  });
function field(name, d) {
  return ui.row(
    {
      h: 34,
      w: "full",
      px: 10,
      items: "center",
      overflow: "hidden",
      radius: 6,
      bg: "code",
      border: 1,
      borderColor: d.editing === name ? "accent" : "line",
    },
    [ui.slot(name)],
  );
}
function miniature(doc) {
  const p = palette(doc);
  return ui.row(
    { h: 86, w: "full", radius: 8, overflow: "hidden", bg: p.pane },
    [
      ui.box({ w: 28, h: "full", shrink: 0, bg: p.rail, p: 7 }, [
        ui.box({ w: 12, h: 12, radius: 3, bg: p.accent }),
      ]),
      ui.col({ grow: 1, p: 10, gap: 7 }, [
        ui.box({ h: 16, ml: 24, radius: 4, bg: p.selected }),
        ui.box({ h: 4, w: 68, radius: 2, bg: p.fg }),
        ui.box({ h: 4, w: 45, radius: 2, bg: p.fg_muted }),
        ui.box({ h: 16, radius: 5, bg: p.raised }),
      ]),
    ],
  );
}
function preview(doc) {
  const p = palette(doc);
  return ui.col(
    {
      id: "theme-preview",
      p: 18,
      radius: 12,
      bg: p.pane,
      color: p.fg,
      gap: 12,
    },
    [
      ui.row({ justify: "end" }, [
        ui.text("This is how your messages will look.", {
          px: 12,
          py: 8,
          radius: 10,
          bg: p.selected,
        }),
      ]),
      ui.text("A reply from your agent."),
      ui.text("Secondary text, paths and timestamps", {
        color: p.fg_muted,
        size: 12,
      }),
      row([
        ui.text("Link", { color: p.accent }),
        ui.text("Success", { color: p.ok }),
        ui.text("Needs you", { color: p.blocked }),
        ui.text("Error", { color: p.error }),
      ]),
      row(
        [
          ui.text("Message your agent…", { grow: 1, color: p.fg_muted }),
          ui.text("Send", {
            px: 12,
            py: 5,
            radius: 6,
            bg: p.primary,
            color: p.primary_ink,
          }),
        ],
        { bg: p.raised, radius: 10, p: 10 },
      ),
    ],
  );
}
function library(d) {
  return [
    heading("Choose your theme"),
    caption("A look for your workspace, conversations and terminals."),
    row(
      d.presets.map((preset, n) =>
        ui.col(
          {
            id: "theme-preset-" + n,
            key: preset.base,
            w: 196,
            minW: 150,
            grow: 1,
            p: 5,
            radius: 12,
            border: 2,
            borderColor: preset.selected ? "accent" : "line",
            cursor: "pointer",
            focusable: true,
            ring: { borderColor: "accent" },
            onClick: (api) => api.act("theme.select", { preset: preset.base }),
          },
          [
            miniature(preset),
            row(
              [
                ui.text(preset.name),
                ui.text(preset.selected ? "Selected" : ""),
              ],
              { justify: "between", px: 6, py: 8 },
            ),
          ],
        ),
      ),
      { gap: 12 },
    ),
    row([
      action("theme-create", "Create a theme", "theme.create", {}, true),
      action("theme-share", "Copy theme link", "theme.share"),
      action("theme-import", "Import from link", "theme.import"),
      action(
        "theme-system",
        d.system ? "Following system" : "Follow system",
        "theme.select",
        { system: true },
      ),
    ]),
    d.saved.length > 0 && heading("Your themes"),
    d.saved.map((saved) =>
      row(
        [
          ui.box({
            w: 24,
            h: 24,
            radius: 6,
            bg: hex(saved.theme.colors.pane),
            border: 2,
            borderColor: hex(saved.theme.colors.accent),
          }),
          ui.text(saved.theme.name, { grow: 1, minW: 0, truncate: true }),
          action(
            "theme-use-" + saved.id,
            saved.selected ? "Active" : "Apply",
            "theme.select",
            { id: saved.id },
          ),
          action("theme-edit-" + saved.id, "Edit", "theme.edit", {
            id: saved.id,
          }),
          action("theme-delete-" + saved.id, "Delete", "theme.delete", {
            id: saved.id,
          }),
        ],
        { key: saved.id, py: 8, borderB: 1, borderColor: "line" },
      ),
    ),
    d.importing && [
      heading("Import a shared theme"),
      caption("Paste a kou-theme:// link. You can preview it before applying."),
      field("link", d),
      row([
        action(
          "theme-import-preview",
          "Preview theme",
          "theme.preview",
          {},
          true,
        ),
        action("theme-import-cancel", "Cancel", "theme.cancel"),
      ]),
    ],
  ];
}
function editor(d) {
  const doc = d.draft.document;
  const group = Math.max(
    0,
    groups.findIndex(([, tokens]) =>
      tokens.some(([token]) => token === d.token),
    ),
  );
  return [
    heading(d.draft.id ? "Edit theme" : "New theme"),
    caption("Name"),
    field("name", d),
    caption("Preview · changes stay here until you save"),
    preview(doc),
    row(
      groups.map(([name, tokens], n) =>
        ui.button(name, {
          key: name,
          selected: n === group,
          onClick: (api) => api.act("theme.token", { token: tokens[0][0] }),
        }),
      ),
    ),
    row(
      groups[group][1].map(([token, name]) =>
        ui.row(
          {
            id: "theme-token-" + token,
            key: token,
            h: 32,
            px: 8,
            gap: 6,
            items: "center",
            radius: 6,
            border: 1,
            borderColor: token === d.token ? "accent" : "line",
            cursor: "pointer",
            focusable: true,
            hover: { bg: "hover" },
            ring: { borderColor: "accent" },
            onClick: (api) => api.act("theme.token", { token }),
          },
          [
            ui.text(name),
            ui.box({ w: 14, h: 14, radius: 3, bg: hex(doc.colors[token]) }),
          ],
        ),
      ),
    ),
    row([
      ui.box({ w: 148 }, [field("color", d)]),
      [
        0xffffff, 0x000000, 0x202124, 0x8090a0, 0xff936b, 0x69b5f8, 0x7bcba5,
        0xc5a0ed,
      ].map((color) =>
        ui.box({
          key: String(color),
          w: 28,
          h: 28,
          radius: 7,
          border: 1,
          borderColor: "lineStrong",
          bg: hex(color),
          tooltip: hex(color),
          cursor: "pointer",
          focusable: true,
          ring: { borderColor: "accent" },
          onClick: (api) => api.act("theme.color", { color }),
        }),
      ),
    ]),
    row([
      action("theme-save", "Save & apply", "theme.save", {}, true),
      action("theme-cancel", "Cancel", "theme.cancel"),
      action("theme-reset", "Reset colours", "theme.reset"),
    ]),
    d.lowContrast &&
      caption(
        "Some text has low contrast. Adjust the text or background before saving.",
      ),
  ];
}
kou.view({
  id: "appearance",
  render(ctx) {
    const d = ctx.data;
    return ui.col({ maxW: 760, gap: d.draft ? 14 : 16 }, [
      d.draft ? editor(d) : library(d),
      d.message && caption(d.message),
      d.problem && ui.text(d.problem, { color: "error" }),
    ]);
  },
});
