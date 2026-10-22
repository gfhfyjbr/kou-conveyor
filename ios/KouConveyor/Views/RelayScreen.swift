import SwiftUI
import UIKit

/// RelayScreen is the relay the app is paired with and the cockpit behind
/// it: how both are, which workspace the app shows, and pairing anew.
struct RelayScreen: View {
    @Environment(AppModel.self) private var model
    @State private var scanning = false
    @State private var unpairing = false
    @State private var checking = false
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
            .sheet(isPresented: $scanning) {
                ScannerSheet { link in
                    scanning = false
                    Task { await model.pairFromLink(link) }
                }
            }
        }
    }

    // MARK: Sections

    private var header: some View {
        let state = TunnelState(model.link)
        return Section {
            VStack(spacing: 12) {
                PlanetMark(lineWidth: 1.2)
                    .foregroundStyle(state.color)
                    .frame(width: 56, height: 56)
                    .padding(16)
                    .background(state.color.opacity(0.12), in: .circle)
                TunnelChip(state: state)
                Text(headline(state))
                    .font(.headline)
                Text(explanation)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 10)
            .animation(.smooth, value: model.link)
        }
    }

    private func relaySection(_ relay: RelayConfig) -> some View {
        Section {
            LabeledContent("Address", value: relay.address)
            LabeledContent("Encryption", value: encryption(of: relay))
            if let fingerprint = relay.fingerprint, let short = relay.shortFingerprint {
                LabeledContent("Certificate") {
                    Text(short)
                        .font(.system(.body, design: .monospaced))
                }
                .contextMenu {
                    Button("Copy fingerprint", systemImage: "doc.on.doc") {
                        UIPasteboard.general.string = fingerprint
                    }
                }
            }
            if let version = model.health?.version, !version.isEmpty {
                // A build's version is long: one line, its ends kept, and
                // whole when copied.
                LabeledContent("Version") {
                    Text(version)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .contextMenu {
                    Button("Copy version", systemImage: "doc.on.doc") {
                        UIPasteboard.general.string = version
                    }
                }
            }
        } header: {
            Text("Relay")
        } footer: {
            if relay.fingerprint != nil {
                Text("The app trusts only the certificate with this fingerprint: the one the pairing code carried.")
            } else if !relay.isEncrypted {
                Text("Plain http: use it only on a network you trust, such as a tailnet.")
            }
        }
    }

    private var cockpitSection: some View {
        Section {
            LabeledContent("Cockpit") {
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
            if let tunnel = model.tunnel {
                LabeledContent("Tunnel", value: describe(tunnel))
                if let host = tunnel.host, !host.isEmpty {
                    LabeledContent("Host", value: host)
                }
                if let error = tunnel.error, !error.isEmpty {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(.red)
                }
            }
        } header: {
            Text("Cockpit")
        } footer: {
            Text("The cockpit dials out to the relay, so its computer needs no open port. /tunnel in the cockpit starts and stops the tunnel.")
        }
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
            Text("Workspace")
        } footer: {
            Text("Sessions and canvases are this workspace's.")
        }
    }

    private var appearanceSection: some View {
        Section {
            NavigationLink {
                AppearanceScreen()
            } label: {
                LabeledContent {
                    Text(Look.shared.summary)
                        .lineLimit(1)
                } label: {
                    Label("Appearance", systemImage: "paintpalette")
                }
            }
        } footer: {
            Text("The accent, the shade of the greys and the corners: the cockpits' choices, for this iPhone.")
        }
    }

    private var transcriptSection: some View {
        Section {
            Toggle("Animate text as it streams", isOn: $streamMotion)
        } header: {
            Text("Transcript")
        } footer: {
            Text("The agent's answer and thinking show as they are written: new words fade in and a caret ends them. Off, the text just appears. The system's Reduce Motion turns the animation off too.")
        }
    }

    private var backgroundSection: some View {
        Section {
            Toggle("Keep runs connected in the background", isOn: $keepAlive)
            Toggle("Notify when a turn ends", isOn: $notifyTurnEnd)
        } header: {
            Text("Notifications & background")
        } footer: {
            Text("While a run is going, the app holds the connection for up to 20 minutes after you leave it, so it need not reconnect — iOS may end this sooner. The turn-end notice is the app's own.")
        }
    }

    private var pairingSection: some View {
        Section {
            Button("Pair with another relay", systemImage: "qrcode.viewfinder") {
                scanning = true
            }
            Button("Paste a pairing link", systemImage: "doc.on.clipboard") {
                paste()
            }
            Button("Unpair", systemImage: "xmark.circle", role: .destructive) {
                unpairing = true
            }
            .confirmationDialog("Unpair from \(model.relay?.address ?? "the relay")?",
                                isPresented: $unpairing, titleVisibility: .visible) {
                Button("Unpair", role: .destructive) { model.unpair() }
            } message: {
                Text("The app forgets the relay and its token. To come back, scan the pairing code again.")
            }
        } header: {
            Text("Pairing")
        }
    }

    // MARK: Words

    private func headline(_ state: TunnelState) -> String {
        switch state {
        case .up:
            if let workspace = model.workspace { return "Connected to \(workspace.name)" }
            return state.label
        default:
            return state.label
        }
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

/// A workspace of the cockpit, ticked when it is the one shown.
private struct WorkspaceRow: View {
    let workspace: Workspace
    let selected: Bool

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(workspace.name)
                    .lineLimit(1)
                Text(workspace.display ?? workspace.path)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            if let running = workspace.running, running > 0 {
                Label("\(running) running", systemImage: "bolt.fill")
                    .font(.caption)
                    .foregroundStyle(Color.kouAccent)
                    .labelStyle(.titleAndIcon)
            }
            Image(systemName: "checkmark")
                .fontWeight(.semibold)
                .foregroundStyle(Color.kouAccent)
                .opacity(selected ? 1 : 0)
        }
        .contentShape(.rect)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
