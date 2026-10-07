import SwiftUI

struct CanvasRoute: Hashable {
    let workspace: String
    let id: String
    let title: String
}

/// CanvasesScreen is the Canvases tab: the workspace's canvases, and the
/// canvas opened from them.
struct CanvasesScreen: View {
    @Environment(AppModel.self) private var app
    @State private var path: [CanvasRoute] = []

    var body: some View {
        NavigationStack(path: $path) {
            CockpitGate { client, workspace in
                CanvasList(client: client, workspace: workspace)
                    .id(workspace.id)
            }
            .navigationTitle("Canvases")
            .navigationDestination(for: CanvasRoute.self) { route in
                if let client = app.client {
                    CanvasScreen(client: client, route: route)
                }
            }
        }
    }
}

struct CanvasList: View {
    @Environment(AppModel.self) private var app
    @State private var model: CanvasesModel
    let workspace: Workspace

    init(client: RelayClient, workspace: Workspace) {
        _model = State(initialValue: CanvasesModel(client: client, workspace: workspace.id))
        self.workspace = workspace
    }

    var body: some View {
        List(model.canvases) { canvas in
            NavigationLink(value: CanvasRoute(workspace: workspace.id, id: canvas.id, title: canvas.title)) {
                CanvasRow(canvas: canvas)
            }
        }
        .overlay {
            if !model.loaded {
                ProgressView()
            } else if let error = model.error, model.canvases.isEmpty {
                ContentUnavailableView("Canvases unavailable", systemImage: "point.3.connected.trianglepath.dotted", description: Text(error))
            } else if model.canvases.isEmpty {
                ContentUnavailableView("No canvases", systemImage: "point.3.connected.trianglepath.dotted",
                                       description: Text("Canvases made in the cockpit show here: terminals and agents, wired together."))
            }
        }
        .refreshable { _ = await model.load() }
        .task { await model.poll(noticed: app.noticed) }
        .navigationSubtitle(workspace.name)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                WorkspaceMenu()
            }
        }
    }
}

struct CanvasRow: View {
    let canvas: CanvasSummary

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: "point.3.filled.connected.trianglepath.dotted")
                .font(.title3)
                .foregroundStyle((canvas.busy ?? 0) > 0 ? Color.kouAccent : .secondary)
                .symbolEffect(.pulse, options: .repeat(.continuous), isActive: (canvas.busy ?? 0) > 0)
                .frame(width: 34, height: 34)
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(canvas.title.isEmpty ? "Untitled canvas" : canvas.title)
                        .font(.body.weight(.medium))
                        .lineLimit(1)
                    if canvas.live == true {
                        Text("LIVE")
                            .font(.system(size: 9, weight: .bold, design: .monospaced))
                            .padding(.horizontal, 5)
                            .padding(.vertical, 2)
                            .background(Color.kouAccent.opacity(0.18), in: .capsule)
                            .foregroundStyle(Color.kouAccent)
                    }
                }
                HStack(spacing: 10) {
                    count(canvas.agents, "sparkle")
                    count(canvas.terminals, "terminal")
                    if let busy = canvas.busy, busy > 0 {
                        Text("\(busy) busy").foregroundStyle(Color.kouAccent)
                    }
                    if let waiting = canvas.waiting, waiting > 0 {
                        Text("\(waiting) waiting").foregroundStyle(.yellow)
                    }
                    if let updated = canvas.updatedAt {
                        Text(updated, format: .relative(presentation: .named))
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 2)
    }

    @ViewBuilder
    private func count(_ value: Int?, _ symbol: String) -> some View {
        if let value, value > 0 {
            Label("\(value)", systemImage: symbol)
                .labelStyle(.titleAndIcon)
        }
    }
}

/// CanvasScreen is one canvas, live: the board, or its nodes as a list, or
/// the messages between them.
struct CanvasScreen: View {
    enum Mode: String, CaseIterable, Identifiable {
        case board = "Board", nodes = "Nodes", messages = "Messages"
        var id: String { rawValue }
    }

    @Environment(AppModel.self) private var app
    @State private var model: CanvasModel
    @State private var mode: Mode = .board
    @State private var opened: CanvasNode?
    let title: String

    init(client: RelayClient, route: CanvasRoute) {
        _model = State(initialValue: CanvasModel(client: client, workspace: route.workspace, id: route.id))
        title = route.title
    }

    var body: some View {
        Group {
            if model.deleted {
                ContentUnavailableView("Canvas deleted", systemImage: "trash", description: Text("It was deleted in the cockpit."))
            } else if model.doc == nil {
                if let error = model.error {
                    ContentUnavailableView("Canvas unavailable", systemImage: "exclamationmark.triangle", description: Text(error))
                } else {
                    ProgressView("Opening the canvas…")
                }
            } else {
                switch mode {
                case .board:
                    CanvasBoard(model: model) { opened = $0 }
                case .nodes:
                    NodeList(model: model) { opened = $0 }
                case .messages:
                    MessageList(model: model)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .safeAreaBar(edge: .bottom) {
            if model.doc != nil {
                Picker("View", selection: $mode) {
                    ForEach(Mode.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .padding(6)
                .glassEffect(.regular, in: .capsule)
                .frame(maxWidth: 360)
                .padding(.bottom, 8)
            }
        }
        .navigationTitle(model.doc?.title ?? title)
        .navigationBarTitleDisplayMode(.inline)
        .navigationSubtitle(subtitle)
        .toolbar(.hidden, for: .tabBar)
        .task { await model.follow() }
        .sheet(item: $opened) { node in
            NodeSheet(model: model, nodeID: node.id)
                .presentationDetents([.medium, .large])
                .presentationBackgroundInteraction(.enabled(upThrough: .medium))
        }
    }

    private var subtitle: String {
        if model.reconnecting { return "Reconnecting…" }
        let busy = model.status.values.count(where: { $0.state == "busy" })
        let waiting = model.status.values.count(where: { $0.state == "waiting" || $0.state == "paused" })
        var parts = ["\(model.nodes.count) nodes"]
        if busy > 0 { parts.append("\(busy) busy") }
        if waiting > 0 { parts.append("\(waiting) waiting") }
        if model.readOnly { parts.append("read-only") }
        return parts.joined(separator: " · ")
    }
}

/// NodeList is the canvas's nodes, for a screen too small for its board.
struct NodeList: View {
    let model: CanvasModel
    var open: (CanvasNode) -> Void

    var body: some View {
        List(model.nodes) { node in
            Button {
                open(node)
            } label: {
                HStack(spacing: 12) {
                    NodeKindIcon(kind: node.kind)
                        .frame(width: 28)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(node.title.isEmpty ? node.kind.capitalized : node.title)
                            .font(.body.weight(.medium))
                            .foregroundStyle(.primary)
                        if let status = model.status[node.id] {
                            Text(status.activity ?? status.detail ?? status.state)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                        }
                    }
                    Spacer()
                    if let status = model.status[node.id] {
                        StateBadge(state: status.state)
                    }
                }
            }
        }
    }
}

/// MessageList is what the canvas's nodes sent each other.
struct MessageList: View {
    let model: CanvasModel

    var body: some View {
        List(model.messages.reversed()) { message in
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 6) {
                    Text(name(message.from?.node))
                    Image(systemName: "arrow.right")
                    Text(name(message.to?.node))
                    Spacer()
                    if let at = message.at {
                        Text(at, format: .dateTime.hour().minute().second())
                    }
                }
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
                if let title = message.title, !title.isEmpty {
                    Text(title).font(.subheadline.weight(.semibold))
                }
                Text(message.text ?? "")
                    .font(.subheadline)
                    .lineLimit(8)
                if let state = message.state, state != "delivered" {
                    Text(message.reason.map { "\(state): \($0)" } ?? state)
                        .font(.caption)
                        .foregroundStyle(.orange)
                }
            }
            .padding(.vertical, 2)
        }
        .overlay {
            if model.messages.isEmpty {
                ContentUnavailableView("No messages yet", systemImage: "arrow.left.arrow.right",
                                       description: Text("What the nodes send each other along their edges shows here."))
            }
        }
    }

    private func name(_ node: String?) -> String {
        guard let node else { return "?" }
        return model.node(node)?.title ?? node
    }
}

struct NodeKindIcon: View {
    let kind: String

    var body: some View {
        Image(systemName: Self.symbol(kind))
            .font(.title3)
            .foregroundStyle(kind == CanvasNode.agent ? Color.kouAccent : .secondary)
    }

    static func symbol(_ kind: String) -> String {
        switch kind {
        case CanvasNode.agent: "sparkle"
        case CanvasNode.terminal: "terminal"
        case CanvasNode.note: "note.text"
        case CanvasNode.source: "antenna.radiowaves.left.and.right"
        default: "square.dashed"
        }
    }
}

struct StateBadge: View {
    let state: String

    var body: some View {
        Text(state)
            .font(.system(size: 10, weight: .semibold, design: .monospaced))
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .foregroundStyle(Self.color(state))
            .background(Self.color(state).opacity(0.15), in: .capsule)
    }

    static func color(_ state: String) -> Color {
        switch state {
        case "busy", "starting": .kouAccent
        case "waiting", "paused": .yellow
        case "error": .red
        case "idle": .green
        default: .secondary
        }
    }
}
