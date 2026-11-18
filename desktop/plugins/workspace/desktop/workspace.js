// The chrome of the workspace, right of the rail: the tab strip, each pane's
// header, the panel a tab is named in, and the header of a split's half that
// waits for its agent. How they look and how they are laid out is all here;
// what is in them is the app's, in `ctx.data`, and what they do is asked of
// the app with `api.act`. The panes' own insides, the lines between them and
// the field a name is typed in stay the app's.

// The key that goes with a control, as a little framed word.
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

// A state's mark, centred in a box of its own so that marks line up.
function markBox(state) {
  return ui.row(
    { shrink: 0, w: 14, h: 14, items: "center", justify: "center" },
    [ui.mark(state)],
  );
}

// The tab strip. `ctx.data`:
//   railHidden, windowButtons   (with the rail hidden, the strip makes room
//                               for the window's buttons and shows it again)
//   session      whether a session is on screen (no tabs or controls if not)
//   width        how wide each tab is
//   tabs         [{ pane, session, index, name, code, icon, active, more,
//                  state }]: `icon` is the one it was given, else null and
//                  the harness's mark (`code`) is shown; `more`, how many
//                  panes besides; `state`, the most pressing of theirs
//   waiting      the tab an agent is yet to be chosen in: null, or
//                { active, closable }
//   splittable   whether the split buttons show
//   panes, canvas   whether the tab is shown as panes, as its canvas
//   newMenu, shell, appKey   the plus's menu: open, whether a shell can be
//                started, the key the app's shortcuts go with
// Acts: rail.show, pane.focus ({ pane }), tab.rename and tab.close
// ({ session, index }), tab.waiting, tab.closeWaiting, split ({ axis:
// "right" | "down" }), layout.set ({ layout: "panes" | "canvas" }),
// newMenu.toggle, newMenu.close, new.agent, new.shell.
kou.view({
  id: "tabs",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    // A tab: as wide as its share of the strip, filled while in front.
    const tab = (key, active, props, children) =>
      ui.row(
        {
          key,
          shrink: 0,
          w: d.width,
          h: m.row,
          // Its mark sits right over the pane header's below it, 12 in
          // from the pane's edge: the strip's 6, the border's 1, and 5.
          pl: 5,
          pr: 4,
          items: "center",
          gap: m.gap + 2,
          radius: m.radius,
          focusable: true,
          cursor: "pointer",
          bg: active ? "selected" : undefined,
          hover: active ? undefined : { bg: "hover" },
          ...props,
        },
        children,
      );
    const ink = (active) => (active ? "fg" : "fgDim");
    const glyph = (name, active) =>
      ui.row({ shrink: 0, w: 16, h: 16, items: "center", justify: "center" }, [
        ui.icon(name, { color: ink(active) }),
      ]);
    const name = (text, active) =>
      ui.text(text, { grow: 1, minW: 0, truncate: true, color: ink(active) });

    const tabs = d.tabs.map((shown) =>
      tab(
        "tab-" + shown.pane,
        shown.active,
        {
          id: "tab-" + shown.pane,
          group: "tab",
          onClick: (api, event) => {
            api.act("pane.focus", { pane: shown.pane });
            // Twice: its name and its icon.
            if (event.count >= 2) {
              api.act("tab.rename", {
                session: shown.session,
                index: shown.index,
              });
            }
          },
        },
        [
          // The icon it was given, else the mark of what runs in it.
          shown.icon
            ? glyph(shown.icon, shown.active)
            : ui.monogram(shown.code, { size: 16 }),
          name(shown.name, shown.active),
          shown.more > 0 &&
            ui.text(String(shown.more + 1), {
              shrink: 0,
              px: 5,
              radius: m.radiusSmall,
              bg: "line",
              size: m.textSmall,
              color: "fgMuted",
            }),
          // Only a state worth a glance gets a mark in the strip.
          shown.state !== "idle" && markBox(shown.state),
          ui.iconButton("close", {
            key: "tab-close-" + shown.pane,
            size: 20,
            tooltip: shown.more > 0 ? "Close the tab and its panes" : "Close",
            stop: true,
            onClick: (api) =>
              api.act("tab.close", {
                session: shown.session,
                index: shown.index,
              }),
          }),
        ],
      ),
    );
    if (d.waiting) {
      const active = d.waiting.active;
      tabs.push(
        tab(
          "tab-waiting",
          active,
          { onClick: (api) => api.act("tab.waiting") },
          [
            glyph("plus", active),
            name("New tab", active),
            d.waiting.closable &&
              ui.iconButton("close", {
                key: "tab-close-waiting",
                size: 20,
                tooltip: "Close",
                stop: true,
                onClick: (api) => api.act("tab.closeWaiting"),
              }),
          ],
        ),
      );
    }

    // What a new tab can be, in a list under the plus.
    const item = (key, icon, label, keys, onClick) =>
      ui.row(
        {
          key,
          position: "relative",
          h: m.row,
          px: m.pad - 1,
          shrink: 0,
          items: "center",
          gap: m.gap,
          radius: m.radius,
          focusable: true,
          cursor: "pointer",
          hover: { bg: "hover" },
          onClick,
        },
        [
          ui.icon(icon, { color: "fgDim" }),
          ui.text(label, { grow: 1, minW: 0, color: "fg" }),
          keyHint(keys, m),
        ],
      );
    const menu =
      d.newMenu &&
      ui.box({ position: "absolute", top: 24 + m.gap, right: 0 }, [
        ui.col(
          {
            key: "new-tab-menu",
            id: "new-tab-menu",
            float: { anchor: "topRight", margin: 8, priority: 1 },
            occlude: true,
            w: 232,
            p: m.unit,
            radius: m.radiusLarge,
            border: 1,
            borderColor: "lineStrong",
            bg: "raised",
            shadow: "lg",
            onOutside: (api) => api.act("newMenu.close"),
          },
          [
            item("new-agent", "thread", "Agent", d.appKey + "+T", (api) =>
              api.act("new.agent"),
            ),
            d.shell &&
              item(
                "new-shell",
                "terminal",
                "Shell",
                d.appKey + "+Option+T",
                (api) => api.act("new.shell"),
              ),
          ],
        ),
      ]);

    const switchTo = (key, icon, tip, layout, active) =>
      ui.iconButton(icon, {
        key,
        tooltip: tip,
        bg: active ? "selected" : undefined,
        onClick: (api) => api.act("layout.set", { layout }),
      });
    const gap = () => ui.box({ w: 6 });
    return ui.row(
      {
        w: "full",
        h: m.bar,
        pl: d.railHidden ? d.windowButtons : 6,
        pr: 8,
        shrink: 0,
        items: "center",
        gap: 2,
        position: "relative",
        bg: "rail",
        // The strip and the rail are one surface, with one grain, and the
        // window is moved by either.
        windowArea: "drag",
      },
      [
        ui.el("grain"),
        d.railHidden &&
          ui.iconButton("sidebar", {
            key: "rail-show",
            tooltip: "Show the rail",
            onClick: (api) => api.act("rail.show"),
          }),
        ui.row(
          {
            key: "tabs",
            grow: 1,
            minW: 0,
            h: "full",
            items: "center",
            gap: 2,
            overflow: "scrollX",
          },
          tabs,
        ),
        d.session && [
          d.splittable && [
            ui.iconButton("split", {
              key: "split-right",
              tooltip: "Split right",
              onClick: (api) => api.act("split", { axis: "right" }),
            }),
            ui.iconButton("split-down", {
              key: "split-down",
              tooltip: "Split down",
              onClick: (api) => api.act("split", { axis: "down" }),
            }),
            gap(),
          ],
          switchTo(
            "layout-panes",
            "pane",
            "This tab as panes",
            "panes",
            d.panes,
          ),
          switchTo(
            "layout-canvas",
            "canvas",
            "This tab as a canvas",
            "canvas",
            d.canvas,
          ),
          gap(),
          // The plus asks what the new tab is to be: an agent, which is
          // asked something first, or a shell.
          ui.box({ position: "relative" }, [
            ui.iconButton("plus", {
              id: "agent-new",
              key: "agent-new",
              tooltip: d.newMenu ? undefined : "New tab",
              stop: true,
              onClick: (api) => api.act("newMenu.toggle"),
            }),
            menu,
          ]),
        ],
      ],
    );
  },
});

// A pane's header. `ctx.data`: code (the harness's mark), title, focused,
// account (the one it runs under, when not the harness's own), cwd, parent
// (the agent that started it), other ({ icon, label }: the other way to
// open what runs here), closed (its agent is gone), state and stateLabel.
// Acts: pane.switch, pane.restart, pane.close ({ pane }).
kou.view({
  id: "paneHeader",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    // (Its cross stands under the strip's plus: 20 in from the pane's edge.)
    return ui.row(
      { h: 34, pl: 12, pr: 9, shrink: 0, items: "center", gap: 8 },
      [
        ui.monogram(d.code, { size: 16 }),
        ui.text(d.title, {
          shrink: 0,
          weight: "medium",
          color: d.focused ? "fg" : "fgDim",
        }),
        // The account it runs under, where its harness has several: a
        // press moves it to the one with the most left, to go on there.
        d.account != null &&
          ui.row(
            {
              key: "pane-account",
              shrink: 0,
              h: 18,
              px: 6,
              items: "center",
              radius: m.radiusSmall,
              border: 1,
              borderColor: "lineStrong",
              size: m.textSmall,
              color: "fgDim",
              cursor: "pointer",
              hover: { bg: "hover" },
              tooltip:
                "The account it runs under. Press to go on under the one with the most left of its limits.",
              stop: true,
              onClick: (api) => api.act("pane.account", { pane: d.pane }),
            },
            [d.account],
          ),
        ui.text(d.cwd, { grow: 1, minW: 0, truncate: true, color: "fgMuted" }),
        // Gives way before the state does when a pane is narrow.
        d.parent != null &&
          ui.text(d.parent, {
            shrink: 1,
            minW: 0,
            truncate: true,
            size: m.textSmall,
            color: "fgMuted",
          }),
        d.other &&
          ui.iconButton(d.other.icon, {
            key: "pane-switch",
            size: 22,
            tooltip: d.other.label,
            stop: true,
            onClick: (api) => api.act("pane.switch", { pane: d.pane }),
          }),
        // A conversation whose agent is gone keeps its transcript on screen;
        // the way to bring the agent back is here.
        d.closed &&
          ui.button("Start again", {
            key: "restart-thread",
            small: true,
            onClick: (api) => api.act("pane.restart", { pane: d.pane }),
          }),
        d.state !== "idle" && [
          ui.text(d.stateLabel, {
            shrink: 0,
            size: m.textSmall,
            color: "fgMuted",
          }),
          markBox(d.state),
        ],
        ui.iconButton("close", {
          id: "pane-close-" + d.pane,
          key: "pane-close",
          size: 22,
          tooltip: "Close the pane",
          stop: true,
          onClick: (api) => api.act("pane.close", { pane: d.pane }),
        }),
      ],
    );
  },
});

// The panel under a tab where it is named and given an icon. `ctx.data`:
// left (where it stands), code (the harness's mark, the icon it has when
// given none), icons ([{ key, icon }]), chosen (a key, or null for none).
// One slot, `field`, where the name is typed. Act: tabEditor.icon ({ icon }).
kou.view({
  id: "tabEditor",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const choice = (key, glyph, tip) => {
      const chosen = d.chosen === key;
      return ui.row(
        {
          id: "tab-icon-" + (key || "auto"),
          key: "tab-icon-" + (key || "auto"),
          w: 30,
          h: 30,
          items: "center",
          justify: "center",
          radius: m.radius,
          border: 1,
          borderColor: chosen ? "accent" : "transparent",
          bg: chosen ? "selected" : undefined,
          hover: chosen ? undefined : { bg: "hover" },
          cursor: "pointer",
          tooltip: tip,
          stopPress: true,
          onClick: (api) => api.act("tabEditor.icon", { icon: key }),
        },
        [glyph],
      );
    };
    return ui.col(
      {
        id: "tab-editor",
        key: "tab-editor",
        position: "absolute",
        top: 4,
        left: d.left + 6,
        w: 268,
        p: m.pad,
        gap: m.pad,
        radius: m.radiusLarge,
        border: 1,
        borderColor: "lineStrong",
        bg: "raised",
        shadow: "lg",
        // A press inside it is not a press outside it.
        stopPress: true,
      },
      [
        ui.slot("field"),
        ui.row({ wrap: true, gap: 2 }, [
          choice(null, ui.monogram(d.code, { size: 16 }), "What runs in it"),
          d.icons.map((given) =>
            choice(
              given.key,
              ui.icon(given.icon, { color: "fgDim" }),
              given.key,
            ),
          ),
        ]),
        ui.row(
          { items: "center", gap: 6, size: m.textSmall, color: "fgMuted" },
          [
            keyHint("Enter", m),
            "to keep",
            keyHint("Esc", m),
            "to leave as it was",
          ],
        ),
      ],
    );
  },
});

// What the window has to say (a plugin that broke, a key to press again to
// quit, ...), along the foot of the workspace. `ctx.data`: text.
kou.view({
  id: "notice",
  render(ctx) {
    return ui.text(ctx.data.text, {
      id: "notice",
      shrink: 0,
      px: 12,
      py: 6,
      borderT: 1,
      borderColor: "line",
      size: ctx.theme.metric.textSmall,
      color: "error",
    });
  },
});

// The half of a split that waits for its agent: what can be started there
// (the slot `picker`, the app's list), under a header that says so and
// closes it. Act: picker.close.
kou.view({
  id: "choosing",
  render() {
    return ui.col(
      { w: "full", h: "full", bg: "pane", borderT: 1, borderColor: "accent" },
      [
        ui.row({ h: 34, pl: 12, pr: 9, shrink: 0, items: "center", gap: 8 }, [
          ui.icon("plus", { color: "fgMuted" }),
          ui.text("New pane", {
            grow: 1,
            minW: 0,
            truncate: true,
            weight: "medium",
            color: "fg",
          }),
          ui.iconButton("close", {
            key: "pane-choosing-close",
            size: 22,
            tooltip: "Close",
            onClick: (api) => api.act("picker.close"),
          }),
        ]),
        ui.col(
          {
            key: "pane-choosing-list",
            grow: 1,
            minH: 0,
            overflow: "scroll",
            items: "center",
            pt: 24,
            pb: 16,
          },
          [ui.slot("picker")],
        ),
      ],
    );
  },
});

// A tooltip's body: what a control without a label is for, under the
// pointer. `ctx.data`: text. Drawn at the zoom of what it is the tip of.
kou.view({
  id: "tip",
  render(ctx) {
    const m = ctx.theme.metric;
    return ui.text(ctx.data.text, {
      id: "tip",
      px: 8,
      py: 4,
      radius: m.radius,
      bg: "selected",
      border: "1px",
      borderColor: "lineStrong",
      font: "ui",
      size: m.textSmall,
      color: "fg",
    });
  },
});

// A pane: its header (`ui.slot("header")`) over what runs in it
// (`ui.slot("body")`). `ctx.data`: line (among several panes, whether this
// one has the keys; null when it is alone), which a line along its top says.
kou.view({
  id: "paneFrame",
  render(ctx) {
    const line = ctx.data.line;
    return ui.col(
      {
        w: "full",
        h: "full",
        bg: "pane",
        borderT: line == null ? undefined : "1px",
        borderColor: line
          ? "accent"
          : line === false
            ? "transparent"
            : undefined,
      },
      [ui.slot("header"), ui.slot("body")],
    );
  },
});

// Cached rail/tab entities, the pane engine and the resize gesture are native
// slots. Window layout and overlays can be changed here without a Rust build.
kou.view({
  id: "shell",
  render(ctx) {
    const d = ctx.data;
    return ui.row(
      {
        id: "workspace-shell",
        position: "relative",
        w: "full",
        h: "full",
        bg: "rail",
        color: "fg",
      },
      [
        d.rail > 0 &&
          ui.box({ w: d.rail, h: "full", shrink: 0 }, [ui.slot("rail")]),
        ui.col({ grow: 1, minW: 0, h: "full" }, [
          ui.slot("tabs"),
          ui.box({ grow: 1, minH: 0, position: "relative" }, [
            ui.slot("measure"),
            ui.box({ w: "full", h: "full" }, [ui.slot("content")]),
            d.editor &&
              ui.box(
                {
                  id: "tab-editor-away",
                  position: "absolute",
                  inset: 0,
                  onPress: (api) => api.act("tabEditor.commit"),
                },
                [ui.slot("editor")],
              ),
          ]),
          d.notice && ui.slot("notice"),
        ]),
        d.rail > 0 &&
          ui.box(
            {
              position: "absolute",
              top: 0,
              left: d.rail - 4,
              w: 8,
              h: "full",
              hover: { bg: "alpha(accent, 0.12)" },
            },
            [ui.slot("railResize")],
          ),
      ],
    );
  },
});

kou.view({
  id: "pluginPage",
  render(ctx) {
    const d = ctx.data;
    return ui.box(
      {
        id: "page-" + d.page,
        w: "full",
        h: "full",
        position: "relative",
        overflow: "scroll",
        bg: "pane",
      },
      [
        ui.el("grain"),
        d.problems.length > 0 &&
          ui.col(
            {
              id: "plugin-problems",
              mx: 32,
              mt: 16,
              maxW: 856,
              px: 12,
              py: 10,
              radius: ctx.theme.metric.radiusMedium,
              border: 1,
              borderColor: "alpha(error, 0.5)",
              bg: "raised",
              gap: 6,
              font: "mono",
              size: ctx.theme.metric.textSmall,
              color: "fgDim",
            },
            d.problems.map((text) => ui.text(text)),
          ),
        d.body && ui.slot("body"),
      ],
    );
  },
});
