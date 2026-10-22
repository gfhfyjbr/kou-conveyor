# kou-conveyor for iPhone

The iPhone app of kou-conveyor: the cockpit's sessions, canvases, files and
accounts, live, from anywhere. It reaches the cockpit through
`kou-conveyor-relay`, a small server on a host both can reach. The cockpit
connects out to the relay, so the cockpit itself never listens on a public
address.

## Build

You need Xcode 26 (the iOS 26 SDK) and [XcodeGen](https://github.com/yonaskolb/XcodeGen)
(`brew install xcodegen`). The Xcode project is generated from `project.yml`,
and it is not kept in git:

```sh
cd ios
xcodegen generate
open KouConveyor.xcodeproj
```

For the simulator, just run it. For a device, choose your team under the
target's Signing & Capabilities. If the bundle identifier is taken, change
`bundleIdPrefix` in `project.yml` and generate again.

To build from the command line:

```sh
xcodebuild -project KouConveyor.xcodeproj -scheme KouConveyor \
  -destination 'generic/platform=iOS Simulator' -derivedDataPath build build
```

## Pair with the cockpit

In the cockpit, type `/tunnel`:

- If a relay is set up already and it answers, the tunnel starts, and the
  dialog shows the pairing QR code.
- If not, the agent takes over. It asks which host to install the relay on,
  installs it there over ssh (`kou-conveyor-web tunnel install user@host`),
  and the cockpit connects to it by itself. The dialog with the QR code then
  opens on its own.

To pair, scan the code with the iPhone's camera and tap **Pair**. The app
keeps the relay's address, its token (in the Keychain) and the fingerprint
of its certificate. It trusts only the certificate with that fingerprint.

If you install the relay again on the same host, it keeps its token and
certificate, so the phone stays paired. If the relay is set up anew, scan
the new code. The app asks whether to pair again.

From a terminal, the same steps are:

```sh
kou-conveyor-web tunnel install user@host   # or a Host of ~/.ssh/config
kou-conveyor-web tunnel status              # is the relay there, does it answer
kou-conveyor-web tunnel link                # the pairing link and its QR code
kou-conveyor-web tunnel start | stop        # carry the cockpit, or no longer
```

You can also pair without a camera: enter the relay's address, its token and
its fingerprint on the app's pairing screen. All three are in the link that
`tunnel link` prints, and `kou-conveyor-relay -print-config` on the host
prints them too.

The relay's port (8420 by default) must be open to the phone, both in the
host's firewall and in its provider's security group.

## What it does

- **Sessions.** The transcript as the agent writes it, with tool calls,
  reasoning and images.
  - Send a prompt, stop a run, or queue messages behind a run. A queued
    message can be edited, put to the agent at once, or dropped, and the
    queue can be paused, resumed or cleared.
  - Choose the model (from the workspace's models, searchable), the effort,
    the tool profile and the sandbox.
  - Attach photos, or paste images.
  - See the pictures the agent looked at, under its ViewImage calls. Tap
    one, or a prompt's image, to see it large over the dimmed transcript,
    as the cockpit shows it; a tap anywhere puts it back.
  - Rename, pin, duplicate or delete a session, branch one from any prompt,
    and see its token usage.
  - Open what a prompt changed in the workspace, with diffs.
  - Swipe right on the sessions for all the workspaces, the most recently
    active first, each with its latest session and how many are running;
    tap one to open its sessions, or hold it to start a session there.
- **Canvases.** The board, live, with its nodes and wires, and lists of its
  nodes, its messages and its log.
  - Create a canvas (empty or from a template), rename it, pause it or let
    it go live, change its settings (what its agents may add, its limits),
    save it as a template, or delete it, with or without its agents'
    sessions. Ask its foreman for nodes.
  - A kou agent's node opens its whole chat, as a session's, with the
    node's model and its details a tap away. One that never ran starts with
    its first prompt, which brings it the canvas's brief.
  - Add agents, existing sessions, terminals of the installed harnesses,
    sources and notes; rename, set up, wire, give access to or remove them
    (a removal can be undone for a few seconds).
  - A terminal shows its screen and takes keys and commands; a manual
    source fires; a webhook shows its address; a note is edited.
  - Messages that wait for you are sent as they are or edited, or dropped;
    nodes that agents proposed are approved or discarded. A wire can make
    its messages wait for you, go at once, or stop, with a template.
- **Files.** Browse and search the workspace, with git status. Files open
  highlighted; pictures open as pictures.
- **Accounts.** The accounts gateway's accounts and endpoints, with their
  state, limits, cooldowns, uptime, spend and errors.
  - Refresh an account's limits, and turn accounts and endpoints on or off.
  - **Usage** shows cost, tokens and requests over time, by model and by
    account, with filters.
- **Relay.** Where the app is connected, and the pairing.

These stay in the browser cockpit for now:

- the cockpit's terminals (a canvas's terminals work here);
- moving a canvas's nodes about its board;
- adding, signing in to and removing accounts and endpoints;
- editing prices;
- reordering pins;
- exporting sessions;
- adding and removing workspaces;
- editing the settings and the connection;
- the plugin and skill lists.
