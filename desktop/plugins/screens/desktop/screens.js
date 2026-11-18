// The screens that stand in for a session's panes: what an empty window
// and a stopped pane say, the list of what can be started, the settings and
// the earlier conversations of a pane. How they look and how they are laid
// out is all here; what is in them is the app's, in `ctx.data`, and what
// they do is asked of the app with `api.act`. The prompt field stays the
// app's (`ui.slot("field")`).
//
// An empty state is one drawing of the thing it is about, what is going on,
// and the one thing to do next. Lists use the rail's row and its rhythm,
// with larger pictures. The full-window screens lie on the pane surface
// with its grain.

// How high a list row that leads with a large picture is.
const LIST_ROW = 36;
// How wide the prompt field of an empty screen is, and how far its frame
// stands in from its edges.
const ASK_WIDTH = 620;
const ASK_INSET = 12;

const parts = (m) => {
  // Where a list's text starts, so headings line up with the rows' pictures.
  const EDGE = m.pad;
  const title = (text, props) =>
    ui.text(text, { size: 15, weight: "medium", color: "fg", ...props });
  const detail = (text, props) =>
    ui.text(text, { maxW: 440, align: "center", color: "fgMuted", ...props });
  const keyHint = (keys) =>
    ui.text(keys, {
      shrink: 0,
      px: 4,
      radius: m.radiusSmall,
      border: 1,
      borderColor: "lineStrong",
      font: "mono",
      size: 10.5,
      color: "fgMuted",
    });
  return {
    EDGE,
    title,
    detail,
    keyHint,
    // A full-window screen: the pane surface and its grain, its content in
    // the middle.
    screen: (props, children) =>
      ui.col(
        {
          w: "full",
          h: "full",
          items: "center",
          justify: "center",
          p: 24,
          position: "relative",
          bg: "pane",
          ...props,
        },
        [ui.el("grain"), children],
      ),
    // An empty state: the drawing, the title under it, what it means, and
    // room for the one action.
    empty: (picture, heading, explanation, action) =>
      ui.col({ items: "center" }, [
        ui.el("picture", { name: picture, color: "fgFaint", mb: 16 }),
        title(heading),
        detail(explanation, { mt: 6 }),
        action,
      ]),
    // What Escape does here, under a screen.
    escape: (does) =>
      ui.row(
        {
          mt: 16,
          items: "center",
          gap: 6,
          size: m.textSmall,
          color: "fgMuted",
        },
        [keyHint("Esc"), does],
      ),
    listHeading: (text) => title(text, { px: EDGE, mb: 8 }),
    // A list's row: the rail's, from the list's edge.
    row: (key, selected, props, children) =>
      ui.row(
        {
          key,
          position: "relative",
          h: LIST_ROW,
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
      ),
  };
};

// The harnesses of this machine as a list: those that can be started, then
// the rest with why not. `ctx.data`: heading, choosing (a click makes one the
// one the prompt field's Enter starts, rather than starting it), scanning,
// ready ([{ id, code, name, version, kind, selected, other, adapter }]:
// `kind` is how it opens, `other` the other way ({ kind, icon, label }),
// `adapter` its chat's adapter when that is not here yet ({ message,
// installable, busy })), accounts ([{ harness, code, name, account,
// accountName, kind, selected }]: the harnesses' other accounts), missing
// ([{ code, name, message }]). Acts: harness.choose and harness.start
// ({ harness, kind, account }), adapter.install ({ harness }).
function picker(d, m) {
  const p = parts(m);
  const list = {
    id: "pick-list",
    w: d.choosing ? "full" : 460,
    maxW: d.choosing ? undefined : "full",
    shrink: 0,
  };
  if (d.empty) {
    // Still looking is not the same as found nothing.
    return ui.col({ ...list, items: "center" }, [
      p.empty(
        "search",
        d.heading,
        d.scanning
          ? "Looking for agent harnesses on this machine..."
          : "No agent harness was found on this machine. Install one (Claude Code, Codex, OpenCode, ...) and check again in Settings.",
      ),
    ]);
  }
  const small = (props) => ({ small: true, ...props });
  const ready = d.ready.map((harness, index) =>
    p.row(
      "pick-" + harness.id,
      harness.selected,
      {
        id: "pick-" + harness.id,
        onClick: (api) =>
          // A shell takes no prompt: chosen, it starts.
          d.choosing && harness.id !== "shell"
            ? api.act("harness.choose", {
                harness: harness.id,
                kind: harness.kind,
              })
            : api.act("harness.start", {
                harness: harness.id,
                kind: harness.kind,
                account: null,
              }),
      },
      [
        ui.monogram(harness.code, { size: 20 }),
        ui.text(harness.name, {
          grow: 1,
          minW: 0,
          truncate: true,
          color: "fg",
        }),
        harness.other &&
          ui.button(
            harness.other.label,
            small({
              key: "pick-other-" + index,
              icon: harness.other.icon,
              stop: true,
              onClick: (api) =>
                api.act(d.choosing ? "harness.choose" : "harness.start", {
                  harness: harness.id,
                  kind: harness.other.kind,
                }),
            }),
          ),
        // Its chat goes through an adapter that is not here yet.
        harness.adapter &&
          ui.button(
            harness.adapter.busy ? "Installing..." : "Install its chat",
            small({
              key: "pick-adapter-" + index,
              icon: "thread",
              tooltip: harness.adapter.message,
              stop: true,
              opacity: harness.adapter.installable ? undefined : 0.5,
              onClick:
                harness.adapter.installable && !harness.adapter.busy
                  ? (api) => api.act("adapter.install", { harness: harness.id })
                  : undefined,
            }),
          ),
        // One column for versions, so the buttons before them line up.
        ui.text(harness.version || "", {
          shrink: 0,
          w: 72,
          truncate: true,
          align: "right",
          size: m.textSmall,
          color: "fgMuted",
        }),
      ],
    ),
  );
  return ui.col(list, [
    d.heading ? p.listHeading(d.heading) : null,
    ready,
    d.missing.length > 0 &&
      ui.text("Not available here", {
        mt: m.unit * 4,
        mb: m.unit,
        px: p.EDGE,
        size: m.textSmall,
        weight: "medium",
        color: "fgMuted",
      }),
    // Under the harnesses, their other accounts: the same program with its
    // home elsewhere.
    d.accounts.map((account) =>
      p.row(
        "pick-account-" + account.account,
        account.selected,
        {
          onClick: (api) =>
            api.act(d.choosing ? "harness.choose" : "harness.start", {
              harness: account.harness,
              kind: account.kind,
              account: account.account,
            }),
        },
        [
          ui.monogram(account.code, { size: 20 }),
          ui.text(account.name, { shrink: 0, color: "fg" }),
          ui.text(account.accountName, {
            grow: 1,
            minW: 0,
            truncate: true,
            color: "fgDim",
          }),
        ],
      ),
    ),
    d.missing.map((harness) =>
      ui.row(
        { h: LIST_ROW, px: p.EDGE, shrink: 0, items: "center", gap: m.gap },
        [
          ui.monogram(harness.code, { size: 20, absent: true }),
          ui.text(harness.name, { shrink: 0, color: "fgMuted" }),
          ui.text(harness.message, {
            grow: 1,
            minW: 0,
            truncate: true,
            align: "right",
            size: m.textSmall,
            color: "fgMuted",
          }),
        ],
      ),
    ),
  ]);
}

kou.view({ id: "picker", render: (ctx) => picker(ctx.data, ctx.theme.metric) });

// Who gets what is typed in the empty page's field, as a button at the
// field's foot that opens the list of what can be started. `ctx.data`: here
// (a session is on screen), harness ({ code, name, way }, or null when there
// is nothing to start: `way` says how it opens when that is not its own
// way), account (the one it runs under, when not its own), open (the list
// is open). Act: askMenu.toggle.
kou.view({
  id: "askChooser",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const said = (text, color) => ui.text(text, { size: m.textSmall, color });
    if (!d.harness) {
      return said(
        d.here ? "No agent to start yet" : "Enter chooses a folder to work in",
        "fgMuted",
      );
    }
    return ui.row(
      {
        key: "ask-harness",
        id: "ask-harness",
        h: m.control,
        pl: 6,
        pr: 6,
        items: "center",
        gap: 6,
        radius: m.radius,
        cursor: "pointer",
        bg: d.open ? "selected" : undefined,
        hover: d.open ? undefined : { bg: "hover" },
        tooltip: "Choose what to start",
        stop: true,
        onClick: (api) => api.act("askMenu.toggle"),
      },
      [
        ui.monogram(d.harness.code, { size: 16 }),
        said(d.harness.name, "fg"),
        d.account != null && said(d.account, "fgDim"),
        d.harness.way != null && said(d.harness.way, "fgMuted"),
        ui.icon("chevron-down", { size: 14, color: "fgMuted" }),
      ],
    );
  },
});

// Nothing has been added yet: the field, and the first thing to do.
// Act: space.add.
kou.view({
  id: "noSpaces",
  render(ctx) {
    const p = parts(ctx.theme.metric);
    return p.screen({}, [
      ui.col({ w: ASK_WIDTH, mb: ctx.theme.metric.unit * 4 }, [
        ui.slot("field"),
      ]),
      p.empty(
        "spaces",
        "Add a project folder",
        "A space is a folder. Its sessions run agents there, side by side or on a canvas.",
        ui.box({ mt: 20 }, [
          ui.button("Choose a folder", {
            key: "first-space",
            icon: "folder-plus",
            primary: true,
            onClick: (api) => api.act("space.add"),
          }),
        ]),
      ),
    ]);
  },
});

// There are spaces, and none of their sessions is on screen.
kou.view({
  id: "noSession",
  render(ctx) {
    const p = parts(ctx.theme.metric);
    return p.screen({}, [
      p.empty(
        "rail",
        "No session is open",
        "Pick one in the rail, or press the plus beside a space to start one there.",
      ),
    ]);
  },
});

// What a pane shows when nothing runs in it, and the way to start it again.
// `ctx.data`: pane, title, reason. Act: pane.restart ({ pane }).
kou.view({
  id: "stopped",
  render(ctx) {
    const d = ctx.data;
    return ui.col(
      {
        w: "full",
        h: "full",
        grow: 1,
        minH: 0,
        items: "center",
        justify: "center",
        p: 24,
      },
      [
        parts(ctx.theme.metric).empty(
          "stopped",
          d.title + " is not running",
          d.reason ||
            "It was open when the app last closed. Its process did not survive that.",
          ui.box({ mt: 20 }, [
            ui.button("Start " + d.title + " again", {
              key: "restart",
              icon: "play",
              primary: true,
              onClick: (api) => api.act("pane.restart", { pane: d.pane }),
            }),
          ]),
        ),
      ],
    );
  },
});

// The picker as a screen of its own: a clean page, the field and nothing
// else, and under it, when its chooser is open, what can be started.
// `ctx.data`: menu (the picker's data, when open), escape (what Escape
// does here, when it does something).
kou.view({
  id: "pickerScreen",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const p = parts(m);
    return p.screen({ keyContext: "Picker" }, [
      ui.col({ w: ASK_WIDTH }, [ui.slot("field")]),
      // Under the field's frame, which stands in from the field's own
      // edges; in a low window it takes what room is left, and scrolls.
      d.menu &&
        ui.box(
          {
            key: "ask-menu",
            mt: m.gap,
            w: ASK_WIDTH - 2 * ASK_INSET,
            minH: 0,
            overflow: "scroll",
            p: m.unit,
            radius: m.radiusLarge,
            border: 1,
            borderColor: "lineStrong",
            bg: "raised",
          },
          [picker(d.menu, m)],
        ),
      d.escape && p.escape(d.escape),
    ]);
  },
});

// A pane's earlier conversations: pick one to go back to it. `ctx.data`:
// pane, heading, sessions (null while they are read; else [{ id, name,
// untitled, running, about }]). Act: session.reopen ({ pane, id }).
kou.view({
  id: "sessions",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const p = parts(m);
    let body;
    if (d.sessions == null) {
      body = ui.box({ w: 540 }, [
        p.listHeading(d.heading),
        p.detail("Reading them...", { align: "left", px: p.EDGE }),
      ]);
    } else if (d.sessions.length === 0) {
      body = p.empty(
        "conversations",
        d.heading,
        "There are none yet. This conversation will be the first.",
      );
    } else {
      body = ui.col(
        { key: "sessions", w: 540, maxH: "full", overflow: "scroll" },
        [
          p.listHeading(d.heading),
          d.sessions.map((session) =>
            p.row(
              "session-" + session.id,
              false,
              {
                onClick: (api) =>
                  api.act("session.reopen", { pane: d.pane, id: session.id }),
              },
              [
                ui.text(session.name, {
                  grow: 1,
                  minW: 0,
                  truncate: true,
                  color: session.untitled ? "fgMuted" : "fg",
                }),
                session.running &&
                  ui.row(
                    {
                      shrink: 0,
                      items: "center",
                      gap: m.gap,
                      size: m.textSmall,
                      color: "working",
                    },
                    [ui.mark("working"), "running"],
                  ),
                ui.text(session.about, {
                  shrink: 0,
                  size: m.textSmall,
                  color: "fgMuted",
                }),
              ],
            ),
          ),
        ],
      );
    }
    return p.screen({ keyContext: "Picker" }, [body, p.escape("to go back")]);
  },
});

// The settings. `ctx.data`: smoothCursor, orchestration (`skills`, `mcp`,
// `off`), notifyDone, notifyAsks, refused (the system lets no notification
// through, and one is asked for), dark, accents ([{ name, color, value,
// chosen }]), scanning, harnesses ([{ id, code, name, ready, version,
// state, variable, own, accounts }]: `variable`, the one that moves its home,
// when it can have accounts of its own; `own`, what is known of its own
// account; and `accounts`, [{ id, name, home, proxy, state, signedOut }]:
// `proxy` as it is shown, `state` what is left of its limits or that
// nobody is signed in), balance (a harness's accounts are one pool), claude
// and codex (each installed CLI's chat transport, `direct` or `adapter`),
// claudeKeep and codexKeep (whether its direct chat runs after closing the app), form
// (the account being added or changed: { harness, editing, problem }, with
// its two fields as the slots `accountName` and `accountProxy`), shortcuts
// ([[keys, what]]).
// Acts: settings.cursor, settings.orchestration ({ way }), settings.notify
// ({ asks }), settings.allowNotices, settings.accent ({ value }),
// settings.scan, settings.balance ({ on }), settings.claude/settings.codex
// ({ direct }), settings.claudeKeep/settings.codexKeep ({ on }),
// account.add ({ harness }),
// account.edit ({ account }), account.signIn ({ account }), account.remove
// ({ account }), account.form.commit, account.form.folder,
// account.form.cancel.
kou.view({
  id: "settings",
  render(ctx) {
    const d = ctx.data;
    const m = ctx.theme.metric;
    const p = parts(m);
    const show = (page) => !d.page || d.page === page;
    // A setting: its picture and name in one column, its value in the next.
    const setting = (icon, name, value) =>
      ui.col({ py: 12, shrink: 0, gap: 8 }, [
        ui.row({ items: "center", gap: 8 }, [
          ui.icon(icon, { color: "fgMuted" }),
          ui.text(name, { color: "fg", weight: "medium" }),
        ]),
        ui.row({ items: "center", wrap: true, gap: 8 }, [value]),
      ]);
    // The columns of a harness row and of a key row start where a
    // setting's value does.
    const valueEdge = m.icon + m.gap + 2;
    const note = (children) =>
      ui.box(
        {
          pl: 0,
          mt: -4,
          mb: m.unit,
          maxW: 560,
          size: m.textSmall,
          color: "fgMuted",
        },
        children,
      );
    const section = (icon, name, props, more) =>
      ui.row(
        {
          h: m.row,
          mt: m.unit * 6,
          shrink: 0,
          items: "center",
          gap: m.gap + 2,
          size: m.text,
          weight: "medium",
          color: "fg",
          ...props,
        },
        [ui.icon(icon, { color: "fgMuted" }), name, more],
      );
    const way = (key, label, chosen) =>
      ui.button(label, {
        id: "settings-together-" + key,
        key: "settings-together-" + key,
        selected: d.orchestration === chosen,
        onClick: (api) => api.act("settings.orchestration", { way: chosen }),
      });
    const notice = (asks) => {
      const on = asks ? d.notifyAsks : d.notifyDone;
      return ui.button(on ? "Notify me" : "Stay quiet", {
        key: asks ? "settings-notify-asks" : "settings-notify-done",
        onClick: (api) => api.act("settings.notify", { asks }),
      });
    };
    const ABOUT = {
      skills:
        "Agents coordinate through a skill that is loaded when they need it.",
      mcp: "Give agents direct access to kou’s tools through MCP.",
      off: "Keep agents independent. You can still direct them from a prompt.",
    };
    const indent = 20 + m.gap + 2;
    const pool = (key, label, on) =>
      ui.button(label, {
        key: "settings-balance-" + key,
        selected: d.balance === on,
        onClick: (api) => api.act("settings.balance", { on }),
      });
    const chatSettings = (id, name, description) => {
      if (!d[id]) return [];
      const button = (key, label, direct) => ui.button(label, {
        key: "settings-" + id + "-" + key,
        selected: (d[id] === "direct") === direct,
        onClick: (api) => api.act("settings." + id, { direct }),
      });
      const kept = d[id + "Keep"];
      const keep = (key, label, on) => ui.button(label, {
        key: "settings-" + id + "-keep-" + key,
        selected: kept === on,
        onClick: (api) => api.act("settings." + id + "Keep", { on }),
      });
      const row = (buttons) => ui.row({ items: "center", wrap: true, gap: m.gap }, buttons);
      return [
        setting("thread", name + "'s chat", row([
          button("direct", "Directly", true), button("adapter", "Through its adapter", false),
        ])),
        note([d[id] === "direct" ? description : "Connect " + name + " through its ACP adapter.",
          ui.box({ mt: 2 }, ["For chats opened from now on."])]),
        d[id] === "direct" && kept != null && setting("clock", "With the app closed", row([
          keep("on", "Keeps working", true), keep("off", "Stops", false),
        ])),
        d[id] === "direct" && kept != null && note([kept
          ? name + " keeps working after you close kou. Its chats reconnect when you return."
          : name + " stops when kou closes. Chats resume from saved history."]),
      ];
    };
    // The account being added or changed: its name and proxy, and what
    // takes them.
    const form = (harness) =>
      ui.col({ pl: indent, mt: 2, mb: m.unit * 2, shrink: 0, gap: 6 }, [
        ui.row({ items: "center", gap: m.gap + 2 }, [
          ui.row(
            {
              w: 132,
              shrink: 0,
              h: m.control,
              px: 8,
              items: "center",
              radius: m.radius,
              border: 1,
              borderColor: d.form.onProxy ? "lineStrong" : "accent",
              bg: "pane",
            },
            [ui.slot("accountName")],
          ),
          ui.row(
            {
              grow: 1,
              minW: 0,
              h: m.control,
              px: 8,
              items: "center",
              radius: m.radius,
              border: 1,
              borderColor: d.form.onProxy ? "accent" : "lineStrong",
              bg: "pane",
            },
            [ui.slot("accountProxy")],
          ),
        ]),
        ui.text(
          d.form.problem ||
            "A proxy is optional: host:port:login:password, or http://login:password@host:port. Everything started under the account goes out through it.",
          { size: m.textSmall, color: d.form.problem ? "blocked" : "fgMuted" },
        ),
        ui.row({ items: "center", wrap: true, gap: m.gap }, [
          ui.button(d.form.editing ? "Save" : "Add and sign in", {
            key: "account-form-commit",
            small: true,
            selected: true,
            tooltip: d.form.editing
              ? undefined
              : "Makes its folder, then opens " +
                harness.name +
                "'s own sign-in in a pane, through the proxy",
            onClick: (api) => api.act("account.form.commit"),
          }),
          !d.form.editing &&
            ui.button("Use a folder I have", {
              key: "account-form-folder",
              small: true,
              tooltip:
                "A home that is signed in to already (" +
                harness.variable +
                ")",
              onClick: (api) => api.act("account.form.folder"),
            }),
          ui.button("Cancel", {
            key: "account-form-cancel",
            small: true,
            onClick: (api) => api.act("account.form.cancel"),
          }),
        ]),
      ]);
    const harnesses = d.harnesses.map((harness) => [
      ui.row({ h: 32, shrink: 0, items: "center", gap: m.gap + 2 }, [
        ui.monogram(harness.code, { size: 20, absent: !harness.ready }),
        ui.text(harness.name, {
          w: 132,
          shrink: 0,
          truncate: true,
          color: harness.ready ? "fg" : "fgMuted",
        }),
        ui.text(harness.version || "", {
          w: 96,
          shrink: 0,
          truncate: true,
          font: "mono",
          size: m.textSmall,
          color: "fgMuted",
        }),
        ui.row({ grow: 1, minW: 0, items: "center", gap: m.gap }, [
          harness.ready && ui.icon("check", { color: "ok" }),
          ui.text(harness.state, {
            minW: 0,
            truncate: true,
            size: m.textSmall,
            color: "fgMuted",
          }),
        ]),
      ]),
      // Its other accounts, and the way to add one: the same program with
      // its home in another folder.
      harness.variable && [
        harness.accounts.map((account) =>
          ui.row(
            {
              h: m.row,
              pl: indent,
              shrink: 0,
              items: "center",
              gap: m.gap + 2,
            },
            [
              ui.text(account.name, {
                w: 132,
                shrink: 0,
                truncate: true,
                color: "fgDim",
              }),
              // Where it goes out, when through a proxy; else where it lives.
              ui.text(account.proxy || account.home, {
                grow: 1,
                minW: 0,
                truncate: true,
                font: "mono",
                size: m.textSmall,
                color: "fgMuted",
                tooltip: account.proxy ? account.home : undefined,
              }),
              account.state &&
                ui.text(account.state, {
                  shrink: 0,
                  size: m.textSmall,
                  color: account.signedOut ? "blocked" : "fgMuted",
                }),
              (account.signedOut || !account.state) &&
                ui.button("Sign in", {
                  key: "account-signin-" + account.id,
                  small: true,
                  tooltip: "Its own sign-in, in a pane, through its proxy",
                  onClick: (api) =>
                    api.act("account.signIn", { account: account.id }),
                }),
              ui.iconButton("pencil", {
                key: "account-edit-" + account.id,
                glyph: m.iconSmall ?? 14,
                tooltip: "Its name and its proxy",
                onClick: (api) =>
                  api.act("account.edit", { account: account.id }),
              }),
              ui.iconButton("close", {
                key: "account-remove-" + account.id,
                glyph: m.iconSmall ?? 14,
                tooltip: "Forget this account (its folder is left as it is)",
                onClick: (api) =>
                  api.act("account.remove", { account: account.id }),
              }),
            ],
          ),
        ),
        d.form && d.form.harness === harness.id
          ? form(harness)
          : ui.row(
              { h: m.row, pl: indent, mb: m.unit, shrink: 0, items: "center" },
              [
                ui.button("Add an account", {
                  key: "account-add-" + harness.id,
                  icon: "plus",
                  small: true,
                  tooltip:
                    "Another login of the same program, in a folder of its own, with a proxy of its own if you give one",
                  onClick: (api) =>
                    api.act("account.add", { harness: harness.id }),
                }),
              ],
            ),
      ],
    ]);
    return ui.box(
      {
        id: "settings",
        key: "settings",
        w: "full",
        h: "full",
        position: "relative",
        overflow: "scroll",
        bg: "pane",
      },
      [
        ui.el("grain"),
        ui.col({ maxW: 680, px: 32, pt: 24, pb: 40 }, [
          !d.page ? p.title("Settings", { mb: 8 }) : null,
          show("general")
            ? [
                setting(
                  "cursor",
                  "Terminal cursor",
                  ui.button(d.smoothCursor ? "Smooth" : "Instant", {
                    id: "settings-cursor",
                    key: "settings-cursor",
                    onClick: (api) => api.act("settings.cursor"),
                  }),
                ),
                setting(
                  "branch",
                  "Working together",
                  ui.row({ items: "center", wrap: true, gap: m.gap }, [
                    way("skills", "Skill + command line", "skills"),
                    way("mcp", "MCP tools", "mcp"),
                    way("off", "Off", "off"),
                  ]),
                ),
                note([
                  ABOUT[d.orchestration],
                  ui.box({ mt: 2 }, ["Applies to newly started agents."]),
                ]),
                setting(
                  "lineage",
                  "Accounts",
                  ui.row({ items: "center", wrap: true, gap: m.gap }, [
                    pool("on", "One pool", true),
                    pool("off", "As chosen", false),
                  ]),
                ),
                note([
                  d.balance
                    ? "New agents use the account with the most available capacity. An explicitly selected account always wins."
                    : "Use the account you select, or the last account used for that agent.",
                ]),
                ...chatSettings("claude", "Claude Code", "Run Claude Code directly, with live updates from background commands."),
                ...chatSettings("codex", "Codex", "Use the Codex CLI's built-in app server."),
                setting("check", "When an agent is done", notice(false)),
                setting("eye", "When an agent asks", notice(true)),
                note([
                  "Notify you while you are away from a pane: when your agent finishes or needs an answer.",
                  // Refused by the system, the two above do nothing: said here,
                  // with the way to where it is allowed.
                  d.refused &&
                    ui.row({ mt: m.gap, items: "center", gap: m.gap }, [
                      ui.icon("info", { color: "blocked" }),
                      ui.text(
                        "The system is not letting kou show notifications.",
                        { color: "fg" },
                      ),
                      ui.button("Open System Settings", {
                        key: "settings-notify-allow",
                        onClick: (api) => api.act("settings.allowNotices"),
                      }),
                    ]),
                ]),
              ]
            : null,
          show("accounts")
            ? [
                section("terminal", "Harnesses", {}, [
                  ui.button(d.scanning ? "Checking..." : "Check again", {
                    key: "settings-scan",
                    icon: "search",
                    ml: m.unit,
                    onClick: (api) => api.act("settings.scan"),
                  }),
                ]),
                ui.text(
                  "Found by looking for each command on your PATH and asking it for its version.",
                  { pl: valueEdge, mb: m.unit * 2, color: "fgMuted" },
                ),
                ui.col({ pl: valueEdge }, harnesses),
                d.harnesses.length === 0 &&
                  ui.text(
                    d.scanning
                      ? "Looking for harnesses..."
                      : "None found. Install one and press Check again.",
                    { pl: valueEdge, color: "fgMuted" },
                  ),
              ]
            : null,
          show("shortcuts")
            ? [
                section("keyboard", "Keys", { mb: m.unit }),
                d.shortcuts.map(([keys, what]) =>
                  ui.row(
                    {
                      h: 28,
                      pl: valueEdge,
                      shrink: 0,
                      items: "center",
                      gap: m.gap + 2,
                    },
                    [
                      // Ends where a harness's name column does, so the
                      // descriptions start under the versions and the values.
                      ui.row({ w: 160, shrink: 0 }, [p.keyHint(keys)]),
                      ui.text(what, { color: "fg" }),
                    ],
                  ),
                ),
              ]
            : null,
        ]),
      ],
    );
  },
});

kou.view({
  id: "settingsFrame",
  render(ctx) {
    const page = ctx.data.page;
    const pages = [
      ["appearance", "Appearance"],
      ["general", "General"],
      ["accounts", "Agents & accounts"],
      ["shortcuts", "Keyboard shortcuts"],
    ];
    return ui.col({ id: "settings-frame", w: "full", h: "full", bg: "pane" }, [
      ui.col({ px: 28, pt: 24, pb: 18, shrink: 0 }, [
        ui.text("Settings", { size: 24, weight: "semibold", mb: 6 }),
        ui.text("Make kou work the way you do.", { color: "fgMuted", mb: 18 }),
        ui.row(
          { items: "center", wrap: true, gap: 8 },
          pages.map(([id, title]) =>
            ui.button(title, {
              id: "settings-page-" + id,
              key: id,
              h: 34,
              selected: page === id,
              color: page === id ? "fg" : undefined,
              onClick: (api) => api.act("settings.page", { page: id }),
            }),
          ),
        ),
      ]),
      page === "appearance"
        ? ui.box(
            { id: "appearance-scroll", grow: 1, minH: 0, overflow: "scroll" },
            [ui.box({ p: 28 }, [ui.slot("body")])],
          )
        : ui.box({ grow: 1, minH: 0 }, [ui.slot("body")]),
    ]);
  },
});
