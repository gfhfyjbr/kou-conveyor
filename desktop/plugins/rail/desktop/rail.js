// The rail: the spaces and their sessions on top, every agent by what it is
// doing below, and the pages at its foot. How it looks and how it is laid
// out is all here, and a change to this file shows in the open window as it
// is saved. What is in it is the app's, in `ctx.data`:
//
//   windowButtons  room for the window's own buttons at the top left
//   spaces         [{ id, name, hue, count, collapsed, unfolded, renaming,
//                    busy, sessions, archivedCount, archiveOpen,
//                    archiveUnfolded, archived }]
//                  `unfolded` goes from 0 (folded) to 1 over the app's own
//                  spring, and so does `archiveUnfolded`; the sessions of a
//                  folded space, and of a folded archive, are not listed
//   sessions       [{ id, name, letter, hue, selected, renaming, place, busy }]
//                  (`place`: where it works, when that is not its space's
//                  folder and the rail has room to say so)
//   busy           what the agents under a row are up to, when anything:
//                  { marks: ["working", "blocked"], words }
//   anyPane        whether there is any agent at all
//   agents         [{ pane, title, code, session, age, state, focused }],
//                  the latest change first
//   idleOpen, idleUnfolded   the Idle group's fold
//   usageOpen, settingsOpen, pages: [{ id, title, icon, open }]
//
// One part stays the app's own: `ui.slot("name")`, the field a space's or a
// session's name is typed in while it is renamed (its `renaming`).
//
// What a row asks of the app, with `api.act`: rail.hide, agents.seen,
// agents.idle, space.add, settings.toggle; space.fold, space.archive,
// space.start, space.rename ({ space }); session.select, session.rename,
// session.archive, session.delete ({ session }); pane.focus ({ pane });
// page.show ({ page }).

kou.view({
  id: "rail",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    // Every row's fill starts `inset` in from the rail's edge, and every
    // heading and first picture `inset + pad` in; a level of the tree steps
    // `step` further, which puts a child's picture under its parent's label.
    const EDGE = m.inset + m.pad;
    // An icon button's glyph sits this far inside it: a row of buttons is
    // pulled out by as much, so their glyphs end on the rows' right edge.
    const BUTTON_PAD = 4;

    // A row: a fixed height, a fill from the inset, a hover and a focus ring.
    const row = (key, selected, props, children) =>
      ui.row(
        {
          key,
          position: "relative",
          h: m.row,
          mx: m.inset,
          // (The focus ring's border is part of the padding.)
          px: m.pad - 1,
          shrink: 0,
          items: "center",
          gap: m.gap,
          radius: m.radius,
          focusable: true,
          cursor: "pointer",
          bg: selected ? "selected" : undefined,
          hover: selected ? undefined : { bg: "hover" },
          ...props,
        },
        children,
      );
    // The box a row's leading picture sits in, so that labels line up.
    const seat = (picture) =>
      ui.row({ shrink: 0, w: m.slot, h: m.slot, items: "center", justify: "center" }, [picture]);
    const label = (text, color) => ui.text(text, { grow: 1, minW: 0, truncate: true, color });
    const meta = (text, props) =>
      ui.text(text, {
        shrink: 0,
        maxW: 140,
        truncate: true,
        size: m.textSmall,
        color: "fgMuted",
        ...props,
      });
    // A section's heading, with room on its right for the section's button.
    const heading = (text, button) =>
      ui.row(
        {
          h: m.row,
          pl: EDGE,
          pr: EDGE - BUTTON_PAD,
          shrink: 0,
          items: "center",
          size: m.textSmall,
          weight: "medium",
          color: "fgMuted",
        },
        [ui.text(text, { grow: 1 }), button],
      );
    // What is a detail of a row (what opens a group, a row's own action) is
    // the small icon, in the full icon's box so that the column they stand
    // in stays one.
    const SMALL = m.iconSmall ?? 14;
    const chevron = (open) =>
      ui.row({ shrink: 0, w: m.icon, h: m.icon, items: "center", justify: "center" }, [
        ui.icon(open ? "chevron-down" : "chevron-right", { size: SMALL, color: "fgMuted" }),
      ]);
    // A state's mark, centred in a box of its own so that marks line up.
    const mark = (state) =>
      ui.row({ shrink: 0, w: 14, h: 14, items: "center", justify: "center" }, [ui.mark(state)]);
    // What goes on under a row, with the count in words under the pointer.
    const busyMarks = (key, busy) =>
      busy && ui.row({ key, shrink: 0, items: "center", tooltip: busy.words }, busy.marks.map(mark));
    // How a group folds away: as tall as `open` of it, clipped.
    const folding = (height, open, children) =>
      ui.box(
        {
          shrink: 0,
          w: "full",
          h: Math.max(height * open, 0),
          overflow: "hidden",
          opacity: Math.min(Math.max(open, 0), 1),
        },
        children,
      );
    // A session's letter on its name's hue; quieter, outlined, once archived.
    const letter = (session, quiet) =>
      ui.row(
        {
          shrink: 0,
          w: 18,
          h: 18,
          radius: m.radiusSmall,
          items: "center",
          justify: "center",
          ...(quiet
            ? { border: 1, borderColor: "fgFaint", size: 10.5, weight: "medium", color: "fgMuted" }
            : { bg: session.hue, size: 11, weight: "bold", color: "hueInk" }),
        },
        [ui.text(session.letter)],
      );

    function sessionRow(session, level) {
      if (!session) return null;
      const archived = level === 2;
      const key = "session-" + session.id;
      const group = "session-row-" + session.id;
      const action = ui.iconButton(archived ? "close" : "archive", {
        key: key + (archived ? "-delete" : "-archive"),
        glyph: SMALL,
        tooltip: archived ? "Delete this session" : "Archive this session",
        // A hidden one is no stop for the Tab key.
        tabStop: session.selected,
        stop: true,
        onClick: (api) =>
          api.act(archived ? "session.delete" : "session.archive", { session: session.id }),
      });
      // The row's one action: at its end on the selected row; on the others
      // it shows under the pointer, over the end of the row, while the place
      // hides, so a quiet row keeps all its room.
      const holder = session.selected
        ? ui.box({ shrink: 0, mr: -BUTTON_PAD }, [action])
        : ui.row(
            {
              position: "absolute",
              top: 0,
              bottom: 0,
              right: m.pad - 1 - BUTTON_PAD,
              pl: m.gap,
              items: "center",
              bg: "hover",
              invisible: true,
              groupHover: { group, visible: true },
            },
            [action],
          );
      return row(
        key,
        session.selected,
        {
          id: key,
          pl: m.pad - 1 + level * m.step,
          group,
          onClick: (api, event) => {
            if (session.renaming) return;
            api.act("session.select", { session: session.id });
            // Twice: its name.
            if (event.count >= 2) api.act("session.rename", { session: session.id });
          },
        },
        [
          seat(letter(session, archived)),
          session.renaming
            ? ui.slot("name")
            : ui.text(session.name, {
                grow: 1,
                shrink: 1,
                basis: "auto",
                minW: 0,
                truncate: true,
                color: archived ? "fgMuted" : "fg",
              }),
          // The place gives way long before the name does.
          session.place != null &&
            meta(session.place, {
              shrink: 64,
              minW: 0,
              maxW: "60%",
              groupHover: session.selected ? undefined : { group, visible: false },
            }),
          archived ? null : busyMarks(key + "-busy", session.busy),
          holder,
        ],
      );
    }

    // A space's row, and under it, folding as one block, its sessions, its
    // Archive row and, when that is open, its archived sessions.
    function spaceBlock(space, index) {
      const header = row(
        "space-" + space.id,
        false,
        {
          onClick: (api, event) => {
            if (space.renaming) return;
            // A click folds it or unfolds it; the second of two puts it
            // back as it was and opens its name instead.
            api.act("space.fold", { space: space.id });
            if (event.count >= 2) api.act("space.rename", { space: space.id });
          },
        },
        [
          seat(ui.icon("folder-fill", { color: space.hue })),
          space.renaming ? ui.slot("name") : label(space.name, "fg"),
          busyMarks("space-busy-" + space.id, space.busy),
          ui.iconButton("plus", {
            id: "space-new-" + space.id,
            key: "space-new-" + space.id,
            glyph: SMALL,
            tooltip: "Start a session in this space",
            stop: true,
            onClick: (api) => api.act("space.start", { space: space.id }),
          }),
          meta(String(space.count)),
          chevron(!space.collapsed),
        ],
      );
      const block = [header];
      if (space.unfolded > 0.001) {
        const inner = [];
        let natural = 0;
        for (const session of space.sessions) {
          inner.push(sessionRow(session, 1));
          natural += m.row;
        }
        if (space.archivedCount > 0) {
          inner.push(
            row(
              "archive-" + space.id,
              false,
              {
                pl: m.pad - 1 + m.step,
                onClick: (api) => api.act("space.archive", { space: space.id }),
              },
              [
                seat(ui.icon("archive", { color: "fgMuted" })),
                label("Archive", "fgMuted"),
                meta(String(space.archivedCount)),
                chevron(space.archiveOpen),
              ],
            ),
          );
          natural += m.row;
          if (space.archiveUnfolded > 0.001) {
            const height = space.archivedCount * m.row;
            // The hairline the archived rows hang from, down from the middle
            // of the Archive row's picture; first, so a hovered row covers it.
            const guide = ui.box({
              position: "absolute",
              top: 0,
              bottom: m.unit,
              left: EDGE + m.step + m.slot / 2 - 0.5,
              w: 1,
              bg: "lineStrong",
            });
            inner.push(
              folding(height, space.archiveUnfolded, [
                ui.col({ position: "relative" }, [
                  guide,
                  space.archived.map((session) => sessionRow(session, 2)),
                ]),
              ]),
            );
            natural += height * space.archiveUnfolded;
          }
        }
        block.push(folding(natural, space.unfolded, [ui.col({}, inner)]));
      }
      // Spaces are told apart by a little air as well as by their folders.
      return ui.col({ mt: index > 0 ? m.unit : undefined }, block);
    }

    const spaces = ui.col({ pb: m.unit * 2 }, [
      heading(
        "Spaces",
        ui.iconButton("folder-plus", {
          key: "space-add",
          tooltip: "Add a project folder as a space",
          onClick: (api) => api.act("space.add"),
        }),
      ),
      d.spaces.length === 0
        ? ui.text("No spaces yet. Add a project folder to start.", {
            pl: EDGE,
            pr: EDGE,
            size: m.textSmall,
            color: "fgMuted",
          })
        : d.spaces.map(spaceBlock),
    ]);

    // Every agent, most urgent first: what waits on the user, what failed,
    // what finished, what runs, what rests. A group's heading carries its
    // state's mark where its rows carry their harness's.
    const GROUPS = [
      ["blocked", "Needs you", "blocked"],
      ["failed", "Failed", "error"],
      ["done", "Done", "done"],
      ["working", "Working", "working"],
      ["idle", "Idle", "fgMuted"],
    ];
    const agentRow = (agent) =>
      row(
        "agent-" + agent.pane,
        agent.focused,
        { onClick: (api) => api.act("pane.focus", { pane: agent.pane }) },
        [
          seat(ui.monogram(agent.code, { size: 16 })),
          // The agent's name gives way after its session's.
          ui.text(agent.title, { shrink: 1, minW: 0, truncate: true, weight: "medium", color: "fg" }),
          agent.session != null
            ? ui.text(agent.session, {
                grow: 1,
                minW: 0,
                truncate: true,
                size: m.textSmall,
                color: "fgMuted",
              })
            : ui.box({ grow: 1, minW: 0, truncate: true, size: m.textSmall, color: "fgMuted" }),
          meta(agent.age),
          mark(agent.state),
        ],
      );
    const agents = [heading("Agents")];
    if (!d.anyPane) {
      agents.push(
        ui.text("Agents you start appear here, grouped by what they are doing.", {
          pl: EDGE,
          pr: EDGE,
          size: m.textSmall,
          color: "fgMuted",
        }),
      );
    }
    let first = true;
    for (const [state, name, color] of d.anyPane ? GROUPS : []) {
      const members = d.agents.filter((agent) => agent.state === state);
      if (members.length === 0) continue;
      const title = [
        seat(ui.mark(state)),
        ui.text(name, { shrink: 0, color }),
        ui.text(String(members.length), { grow: 1, color: "fgMuted" }),
      ];
      // Idle folds away.
      const foldable = state === "idle";
      const head = foldable
        ? row(
            "agents-idle",
            false,
            {
              h: m.heading,
              size: m.textSmall,
              weight: "medium",
              onClick: (api) => api.act("agents.idle"),
            },
            [title, chevron(d.idleOpen)],
          )
        : ui.row(
            {
              h: m.heading,
              mx: m.inset,
              px: m.pad,
              shrink: 0,
              items: "center",
              gap: m.gap,
              size: m.textSmall,
              weight: "medium",
            },
            title,
          );
      agents.push(ui.col({ mt: first ? undefined : m.unit }, [head]));
      first = false;
      const unfolded = foldable ? d.idleUnfolded : 1;
      if (unfolded <= 0.001) continue;
      agents.push(folding(members.length * m.row, unfolded, [ui.col({}, members.map(agentRow))]));
    }

    // The pages, as the settings are: a row shows one, and goes back from it.
    const page = (key, open, glyph, title, onClick) =>
      row(key, open, { onClick }, [seat(ui.icon(glyph, { color: "fgMuted" })), label(title, "fg")]);
    const foot = ui.box({ shrink: 0, py: m.unit, borderT: 1, borderColor: "line" }, [
      page("rail-usage", d.usageOpen, "flame", "Usage", (api) =>
        api.act("page.show", { page: "usage" }),
      ),
      d.pages.map((shown) =>
        page("rail-page-" + shown.id, shown.open, shown.icon, shown.title, (api) =>
          api.act("page.show", { page: shown.id }),
        ),
      ),
      page("rail-settings", d.settingsOpen, "sliders", "Settings", (api) =>
        api.act("settings.toggle"),
      ),
    ]);

    return ui.col(
      {
        id: "rail",
        w: "full",
        h: "full",
        position: "relative",
        bg: "rail",
        borderR: 1,
        borderColor: "line",
      },
      [
        ui.el("grain"),
        // The bar the window is moved by, with the rail's own buttons.
        ui.row(
          {
            h: m.bar,
            pl: d.windowButtons,
            pr: EDGE - BUTTON_PAD,
            shrink: 0,
            items: "center",
            justify: "end",
            gap: 2,
            windowArea: "drag",
          },
          [
            ui.iconButton("check-circle", {
              key: "rail-seen",
              tooltip: "Mark every finished agent as seen",
              onClick: (api) => api.act("agents.seen"),
            }),
            ui.iconButton("sidebar", {
              key: "rail-hide",
              tooltip: "Hide the rail",
              onClick: (api) => api.act("rail.hide"),
            }),
          ],
        ),
        ui.box({ key: "rail-spaces", grow: 1, minH: 0, overflow: "scroll" }, [spaces]),
        ui.col(
          {
            key: "rail-agents",
            shrink: 0,
            maxH: "50%",
            overflow: "scroll",
            pt: m.unit,
            pb: m.unit * 2,
            borderT: 1,
            borderColor: "line",
          },
          agents,
        ),
        foot,
      ],
    );
  },
});
