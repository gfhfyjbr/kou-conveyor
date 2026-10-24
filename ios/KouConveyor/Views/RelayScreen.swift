import SwiftUI
import UIKit

/// RelayScreen is the relay the app is paired with and the cockpit behind
/// it: how both are, which workspace the app shows, and pairing anew.
struct RelayScreen: View {
    @Environment(AppModel.self) private var model
    @State private var scanning = false
    @State private var unpairing = false
    @State private var checking = false
    /// The checks asked for, done: each answers with a tap of the hand.
    @State private var checks = 0
    @AppStorage(BackgroundSettings.keepAliveKey) private var keepAlive = true
    @AppStorage(BackgroundSettings.notifyKey) private var notifyTurnEnd = true
    @AppStorage(StreamSettings.motionKey) private var streamMotion = true

    var body: some View {
        NavigationStack {
            Form {
                header
                if let relay = model.relay {
                    relaySection(relay)
                }
                cockpitSection
                if !model.workspaces.isEmpty {
                    workspaceSection
                }
                appearanceSection
                transcriptSection
                backgroundSection
                pairingSection
            }
            .groupedBackground()
            .listSectionSpacing(22)
            .navigationTitle("Relay")
            .navigationSubtitle(model.relay?.address ?? "")
            .refreshable { await model.refresh() }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Check now", systemImage: "arrow.clockwise") { check() }
                        .symbolEffect(.rotate, isActive: checking)
                        .disabled(checking)
                }
            }
            // A check the user asked for answers in the hand too.
            .sensoryFeedback(trigger: checks) { _, _ in
                TunnelState(model.link) == .up ? .success : .warning
            }
            .sensoryFeedback(.selection, trigger: model.workspace?.id) { old, new in
                old != nil && new != nil
            }
            .sheet(isPresented: $scanning) {
                ScannerSheet { link in
                    scanning = false
                    Task { await model.pairFromLink(link) }
                }
            }
        }
    }

    // MARK: Sections

    /// The tunnel at large: its orbit, how it is, the cockpit it reaches and
    /// the readings that matter — as the cockpit's own tunnel dialog.
    private var header: some View {
        let state = TunnelState(model.link)
        return Section {
            VStack(spacing: 0) {
                Orbit(color: state.color, live: state == .up, size: 150)
                    .padding(.bottom, 10)
                HStack(spacing: 4) {
                    LiveDot(color: state.color, size: 6, live: state == .up || state == .checking)
                    Eyebrow(state == .up ? "Connected to" : "Tunnel", color: state.color)
                }
                Text(verbatim: headline(state))
                    .font(.title2.weight(.heavy).width(.expanded))
                    .foregroundStyle(Color.kouFg)
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
                    .padding(.top, 4)
                Text(verbatim: explanation)
                    .font(.footnote)
                    .foregroundStyle(Color.kouFg3)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 8)
                    .padding(.horizontal, 16)
                readings
                    .padding(.top, 20)
            }
            .frame(maxWidth: .infinity)
            .padding(.bottom, 4)
            .animation(.smooth, value: model.link)
            .listRowBackground(Color.clear)
            .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
        }
    }

    /// What the link is made of, at a glance: how it is secured, how long
    /// the cockpit has been on it, and how much runs through it.
    private var readings: some View {
        ReadingStrip {
            Reading(label: "Link") {
                Text(verbatim: crypto.word)
                    .foregroundStyle(crypto.tone)
            }
            Reading(label: "Uptime") {
                if model.health?.agent == true, let since = model.health?.agentSince {
                    TimelineView(.everyMinute) { _ in
                        Text(verbatim: since.kouAge == "now" ? "<1m" : since.kouAge)
                            .foregroundStyle(Color.kouFg)
                    }
                } else {
                    Text(verbatim: model.health?.agent == true ? "on" : model.health == nil ? "—" : "off")
                        .foregroundStyle(model.health?.agent == true ? Color.kouFg : Color.kouFg3)
                }
            }
            Reading(label: "Running") {
                Text(verbatim: "\(running)")
                    .monospacedDigit()
                    .foregroundStyle(running > 0 ? Color.kouAccent : Color.kouFg3)
                    .contentTransition(.numericText())
                    .animation(.snappy, value: running)
            }
        }
        .padding(.horizontal, 2)
    }

    private func relaySection(_ relay: RelayConfig) -> some View {
        Section {
            value("Address", relay.address)
            LabeledContent("Encryption") {
                Text(verbatim: encryption(of: relay))
                    .foregroundStyle(Color.kouFg2)
            }
            if let fingerprint = relay.fingerprint, let short = relay.shortFingerprint {
                value("Certificate", short)
                    .contextMenu {
                        Button("Copy fingerprint", systemImage: "doc.on.doc") {
                            UIPasteboard.general.string = fingerprint
                        }
                    }
            }
            if let version = model.health?.version, !version.isEmpty {
                // A build's version is long: one line, its ends kept, and
                // whole when copied.
                value("Version", version)
                    .contextMenu {
                        Button("Copy version", systemImage: "doc.on.doc") {
                            UIPasteboard.general.string = version
                        }
                    }
            }
        } header: {
            Eyebrow("Relay")
        } footer: {
            if relay.fingerprint != nil {
                FormNote("The app trusts only the certificate with this fingerprint: the one the pairing code carried.")
            } else if !relay.isEncrypted {
                FormNote("Plain http: use it only on a network you trust, such as a tailnet.")
            }
        }
        .kouRow()
    }

    private var cockpitSection: some View {
        Section {
            LabeledContent("Cockpit") {
                HStack(spacing: 2) {
                    LiveDot(color: model.health?.agent == true ? .kouOK : .kouFg4, size: 6, live: false)
                    Group {
                        if model.health?.agent == true {
                            if let since = model.health?.agentSince {
                                Text("connected for \(Text(since, style: .relative))")
                            } else {
                                Text("connected")
                            }
                        } else if model.health != nil {
                            Text("not connected")
                        } else {
                            Text("unknown")
                        }
                    }
                    .foregroundStyle(Color.kouFg2)
                    .lineLimit(1)
                }
            }
            if let tunnel = model.tunnel {
                LabeledContent("Tunnel") {
                    Tag(describe(tunnel), tone: tunnel.state == "up" ? .kouOK : tunnel.state == "off" ? .kouFg3 : .kouWarn)
                }
                if let host = tunnel.host, !host.isEmpty {
                    value("Host", host)
                }
                if let error = tunnel.error, !error.isEmpty {
                    Label {
                        Text(verbatim: error)
                            .font(.system(.footnote, design: .monospaced))
                            .foregroundStyle(Color.kouErr)
                    } icon: {
                        RowIcon("exclamationmark.triangle.fill", tone: .kouErr)
                    }
                }
            }
        } header: {
            Eyebrow("Cockpit")
        } footer: {
            FormNote("The cockpit dials out to the relay, so its computer needs no open port. /tunnel in the cockpit starts and stops the tunnel.")
        }
        .kouRow()
    }

    private var workspaceSection: some View {
        Section {
            ForEach(model.workspaces) { workspace in
                Button {
                    model.select(workspace: workspace)
                } label: {
                    WorkspaceRow(workspace: workspace, selected: workspace.id == model.workspace?.id)
                }
                .tint(.primary)
            }
        } header: {
            Eyebrow("Workspace")
        } footer: {
            FormNote("Sessions and canvases are this workspace's.")
        }
        .kouRow()
    }

    private var appearanceSection: some View {
        Section {
            NavigationLink {
                AppearanceScreen()
            } label: {
                Label {
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Appearance")
                            .foregroundStyle(Color.kouFg)
                        Text(verbatim: Look.shared.summary)
                            .font(.kouMeta)
                            .foregroundStyle(Color.kouFg3)
                            .lineLimit(1)
                    }
                } icon: {
                    LookMark()
                }
            }
        } header: {
            Eyebrow("This iPhone")
        } footer: {
            FormNote("The accent, the shade of the greys and the corners: the cockpits' choices, for this iPhone.")
        }
        .kouRow()
    }

    private var transcriptSection: some View {
        Section {
            Toggle(isOn: $streamMotion) {
                Label {
                    Text("Animate text as it streams")
                } icon: {
                    RowIcon("character.cursor.ibeam")
                }
            }
        } header: {
            Eyebrow("Transcript")
        } footer: {
            FormNote("The agent's answer and thinking show as they are written: new words fade in and a caret ends them. Off, the text just appears. The system's Reduce Motion turns the animation off too.")
        }
        .kouRow()
    }

    private var backgroundSection: some View {
        Section {
            Toggle(isOn: $keepAlive) {
                Label {
                    Text("Keep runs connected in the background")
                } icon: {
                    RowIcon("antenna.radiowaves.left.and.right")
                }
            }
            Toggle(isOn: $notifyTurnEnd) {
                Label {
                    Text("Notify when a turn ends")
                } icon: {
                    RowIcon("bell.badge")
                }
            }
        } header: {
            Eyebrow("Notifications & background")
        } footer: {
            FormNote("While a run is going, the app holds the connection for up to 20 minutes after you leave it, so it need not reconnect — iOS may end this sooner. The turn-end notice is the app's own.")
        }
        .kouRow()
    }

    private var pairingSection: some View {
        Section {
            Button {
                scanning = true
            } label: {
                Label {
                    Text("Pair with another relay")
                        .foregroundStyle(Color.kouFg)
                } icon: {
                    RowIcon("qrcode.viewfinder", tone: .kouAccent)
                }
            }
            Button {
                paste()
            } label: {
                Label {
                    Text("Paste a pairing link")
                        .foregroundStyle(Color.kouFg)
                } icon: {
                    RowIcon("doc.on.clipboard")
                }
            }
            Button(role: .destructive) {
                unpairing = true
            } label: {
                Label {
                    Text("Unpair")
                        .foregroundStyle(Color.kouErr)
                } icon: {
                    RowIcon("xmark", tone: .kouErr)
                }
            }
            .confirmationDialog("Unpair from \(model.relay?.address ?? "the relay")?",
                                isPresented: $unpairing, titleVisibility: .visible) {
                Button("Unpair", role: .destructive) { model.unpair() }
            } message: {
                Text("The app forgets the relay and its token. To come back, scan the pairing code again.")
            }
        } header: {
            Eyebrow("Pairing")
        }
        .kouRow()
    }

    /// A row of a value the machine wrote — an address, a fingerprint, a
    /// version: in mono, its ends kept when it is too long.
    private func value(_ label: LocalizedStringKey, _ value: String) -> some View {
        LabeledContent(label) {
            Text(verbatim: value)
                .font(.system(.subheadline, design: .monospaced))
                .foregroundStyle(Color.kouFg2)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    // MARK: Words

    private func headline(_ state: TunnelState) -> String {
        state == .up ? model.workspace?.name ?? "the cockpit" : state.label
    }

    private var explanation: String {
        switch model.link {
        case .online:
            "The cockpit is connected to its relay: its sessions, chat and canvases here are live."
        case .cockpitOffline:
            "The relay answers, but no cockpit is connected to it. Run /tunnel in the cockpit, or start kou-conveyor-web with -tunnel on."
        case let .failed(message):
            message
        case .checking:
            "Asking the relay…"
        }
    }

    /// How the link is secured, as the strip reads it.
    private var crypto: (word: String, tone: Color) {
        guard let relay = model.relay else { return ("—", .kouFg3) }
        if !relay.isEncrypted { return ("HTTP", .kouWarn) }
        return relay.fingerprint == nil ? ("TLS", .kouFg) : ("TLS·PIN", .kouOK)
    }

    /// The sessions running in the cockpit's workspaces.
    private var running: Int {
        model.workspaces.reduce(0) { $0 + ($1.running ?? 0) }
    }

    private func encryption(of relay: RelayConfig) -> String {
        if !relay.isEncrypted { return "None (http)" }
        return relay.fingerprint == nil ? "TLS" : "TLS, pinned"
    }

    private func describe(_ tunnel: TunnelStatus) -> String {
        switch tunnel.state {
        case "up": "up"
        case "connecting": "connecting"
        case "retrying": "retrying"
        case "setup": "being set up"
        case "off": "off"
        default: tunnel.state
        }
    }

    // MARK: Actions

    private func check() {
        checking = true
        Task {
            await model.refresh()
            checking = false
            checks += 1
        }
    }

    private func paste() {
        let text = UIPasteboard.general.string?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !text.isEmpty, let url = URL(string: text) else {
            model.linkError = "The clipboard has no pairing link. In the cockpit, the tunnel's dialog copies it."
            return
        }
        model.open(url)
    }
}

/// A workspace of the cockpit: its monogram, ringed when it is the one
/// shown, and what runs in it.
private struct WorkspaceRow: View {
    let workspace: Workspace
    let selected: Bool

    var body: some View {
        HStack(spacing: 12) {
            Monogram(name: workspace.name, size: 30, current: selected)
                .padding(selected ? 0 : 3)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: workspace.name)
                    .font(.body.weight(selected ? .semibold : .regular))
                    .foregroundStyle(Color.kouFg)
                    .lineLimit(1)
                Text(verbatim: workspace.display ?? workspace.path)
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg3)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            if let running = workspace.running, running > 0 {
                HStack(spacing: 2) {
                    LiveDot(size: 6)
                    Text(verbatim: "\(running)")
                        .font(.kouMeta.weight(.semibold))
                        .foregroundStyle(Color.kouAccent)
                }
                .accessibilityLabel("\(running) running")
            }
            Image(systemName: "checkmark")
                .font(.footnote.weight(.bold))
                .foregroundStyle(Color.kouAccent)
                .opacity(selected ? 1 : 0)
        }
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

/// LookMark is the look in a row's mark: the accent on the shade's ground,
/// in the corners chosen.
struct LookMark: View {
    var body: some View {
        let shape = RoundedRectangle(cornerRadius: .corner(7), style: .continuous)
        ZStack {
            shape.fill(Color.kouSurface)
            Circle()
                .fill(Color.kouAccent)
                .frame(width: 11, height: 11)
                .offset(x: 4, y: 4)
            RoundedRectangle(cornerRadius: .corner(2.5), style: .continuous)
                .fill(Color.kouFg3)
                .frame(width: 11, height: 3)
                .offset(x: -3, y: -5)
        }
        .frame(width: 28, height: 28)
        .overlay(shape.strokeBorder(Color.kouLine2.opacity(0.7), lineWidth: 1))
        .accessibilityHidden(true)
    }
}
