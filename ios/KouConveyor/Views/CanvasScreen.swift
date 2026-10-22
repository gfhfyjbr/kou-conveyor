import SwiftUI

/// The chat of a canvas's kou agent.
struct NodeChatRoute: Hashable {
    let workspace: String
    let canvas: String
    let node: String
    let session: String
    /// Nothing ran in the session yet, as far as the canvas knows.
    let fresh: Bool
    /// The model the node is set to.
    let model: String?
    let title: String
}

/// What is done to a node from the board or a list.
enum NodeAction {
    case open(CanvasNode)
    case inspect(CanvasNode)
    case rename(CanvasNode)
    case approve(CanvasNode)
    case discard(CanvasNode)
    case remove(CanvasNode)
}

/// A node to add whose required settings the user gives first.
struct NewNode: Identifiable {
    let id = UUID()
    let kind: String
    let preset: String
    let plugin: String
    let title: String
    let schema: ConfigSchema
}

struct NodeRef: Identifiable, Hashable {
    let id: String
}

/// CanvasScreen is one canvas, live: the board, its nodes as a list, its
/// messages — those that wait for the user first — and what happened on
/// it; and all the user does to it: nodes added, wired, approved, removed,
/// the canvas paused, set, saved as a template or deleted.
struct CanvasScreen: View {
    enum Mode: String, CaseIterable, Identifiable {
        case board = "Board", nodes = "Nodes", messages = "Messages", log = "Log"
        var id: String { rawValue }
    }

    enum Sheet: Identifiable {
        case configure(NewNode), sessions, foreman, settings, template, message(CanvasMessage)

        var id: String {
            switch self {
            case let .configure(node): "configure:\(node.id)"
            case .sessions: "sessions"
            case .foreman: "foreman"
            case .settings: "settings"
            case .template: "template"
            case let .message(message): "message:\(message.id)"
            }
        }
    }

    enum Renaming: Hashable {
        case canvas
        case node(String)
    }

    @Environment(\.dismiss) private var dismiss
    @State private var model: CanvasModel
    @State private var mode: Mode = .board
    @State private var inspected: NodeRef?
    @State private var sheet: Sheet?
    @State private var renaming: Renaming?
    @State private var renameText = ""
    @State private var deletingCanvas = false
    @State private var removing: CanvasNode?
    /// The node to open once the sheet over the canvas is gone.
    @State private var then: String?
    /// The board's scale the user chose, kept while the other views show.
    @State private var boardScale: CGFloat?
    let client: RelayClient
    let title: String
    var openChat: (NodeChatRoute) -> Void
    var openSession: (SessionRoute) -> Void

    init(client: RelayClient, route: CanvasRoute, openChat: @escaping (NodeChatRoute) -> Void,
         openSession: @escaping (SessionRoute) -> Void) {
        _model = State(initialValue: CanvasModel(client: client, workspace: route.workspace, id: route.id))
        self.client = client
        title = route.title
        self.openChat = openChat
        self.openSession = openSession
    }

    var body: some View {
        content
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            // The board's gray, and its lists': on white, the canvas came
            // in white and turned gray at once as it opened.
            .background(Color.kouGroupedGround)
            // Over the board, not in a bar above it: as such a bar went, its
            // height going down took the darkening under the title away, to
            // fade back a moment later.
            .overlay(alignment: .top) { noticeBar }
            .safeAreaBar(edge: .bottom) { modePicker }
            .animation(.snappy, value: model.notice)
            .navigationTitle(model.doc?.title ?? title)
            .navigationBarTitleDisplayMode(.inline)
            .navigationSubtitle(subtitle)
            .toolbar {
                ToolbarItemGroup(placement: .topBarTrailing) {
                    addMenu
                    moreMenu
                }
            }
            .task { await model.live() }
            .task { await model.loadKinds() }
            .navigationDestination(for: NodeChatRoute.self) { route in
                NodeChat(model: model, client: client, route: route, openSession: openSession)
            }
            .sheet(item: $inspected, onDismiss: openPending) { ref in
                NodeInspector(model: model, nodeID: ref.id) { node in
                    then = node.id
                    inspected = nil
                }
                .presentationDetents([.medium, .large])
                .presentationBackgroundInteraction(.enabled(upThrough: .medium))
            }
            .sheet(item: $sheet, onDismiss: openPending) { sheet in
                sheetView(sheet)
            }
            .alert(renaming == .canvas ? "Rename the canvas" : "Rename the node",
                   isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } }),
                   presenting: renaming) { target in
                TextField("Title", text: $renameText)
                Button("Rename") { rename(target) }
                Button("Cancel", role: .cancel) {}
            }
            .confirmationDialog("Delete «\(model.doc?.title ?? title)»?", isPresented: $deletingCanvas, titleVisibility: .visible) {
                Button("Delete, keep its agents' sessions", role: .destructive) { deleteCanvas(keepSessions: true) }
                Button("Delete with its agents' sessions", role: .destructive) { deleteCanvas(keepSessions: false) }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Its terminals stop and its messages go. Sessions kept stay in the sessions list.")
            }
            .confirmationDialog(removing.map { "Remove «\($0.title)»?" } ?? "Remove the node?",
                                isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
                                titleVisibility: .visible, presenting: removing) { node in
                Button("Remove, keep its session", role: .destructive) {
                    Task { await model.remove(node.id) }
                }
                Button("Remove with its session", role: .destructive) {
                    Task { await model.remove(node.id, keepSession: false) }
                }
                Button("Cancel", role: .cancel) {}
            } message: { _ in
                Text("A session kept goes on in the sessions list. For a few seconds the node can be put back.")
            }
    }

    // MARK: Parts

    @ViewBuilder
    private var content: some View {
        if model.deleted {
            ContentUnavailableView("Canvas deleted", systemImage: "trash", description: Text("It is no longer in the cockpit."))
        } else if model.doc == nil {
            if let error = model.error {
                ContentUnavailableView("Canvas unavailable", systemImage: "exclamationmark.triangle", description: Text(error))
            } else {
                LoadingSpinner(title: "Opening the canvas…")
            }
        } else {
            switch mode {
            case .board:
                CanvasBoard(model: model, chosen: $boardScale, act: act)
            case .nodes:
                NodeList(model: model, act: act)
            case .messages:
                CanvasInbox(model: model, act: act) { sheet = .message($0) }
            case .log:
                CanvasLog(model: model)
            }
        }
    }

    @ViewBuilder
    private var modePicker: some View {
        if model.doc != nil, !model.deleted {
            Picker("View", selection: $mode) {
                ForEach(Mode.allCases) { mode in
                    Text(mode == .messages && model.needsYou > 0 ? "\(mode.rawValue) \(model.needsYou)" : mode.rawValue)
                        .tag(mode)
                }
            }
            .pickerStyle(.segmented)
            .padding(6)
            .glassEffect(.regular, in: .cornerCapsule)
            .frame(maxWidth: 440)
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
        }
    }

    @ViewBuilder
    private var noticeBar: some View {
        CanvasNoticeSlot(model: model)
            // On the board, beside the zoom's buttons, not over them.
            .padding(.trailing, mode == .board ? 44 : 0)
    }

    private var subtitle: String {
        if model.reconnecting { return "Reconnecting…" }
        guard model.doc != nil else { return "" }
        let busy = model.status.values.count(where: { $0.state == "busy" })
        let nodes = model.nodes.count
        var parts = [nodes == 1 ? "1 node" : "\(nodes) nodes"]
        if !model.isLive { parts.append("paused") }
        if busy > 0 { parts.append("\(busy) busy") }
        if model.needsYou > 0 { parts.append(model.needsYou == 1 ? "1 needs you" : "\(model.needsYou) need you") }
        if model.readOnly { parts.append("read-only") }
        return parts.joined(separator: " · ")
    }

    private var addMenu: some View {
        // Terminals and sources open from their own entries: all of them in
        // one list do not fit a phone's screen.
        Menu {
            Section {
                Button("Kou agent", systemImage: "sparkle") {
                    add(["kind": "agent", "title": .string(model.freeTitle("Agent"))])
                }
                Button("Session…", systemImage: "text.bubble") { sheet = .sessions }
            }
            Section {
                Menu("Terminal", systemImage: "terminal") {
                    if model.kinds == nil {
                        Button("Shell", systemImage: "terminal") {
                            add(["kind": "terminal", "preset": "shell", "title": .string(model.freeTitle("Shell"))])
                        }
                    }
                    ForEach(harnesses, id: \.self) { kind in
                        Button(kind.name, systemImage: Self.symbol(harness: kind.id)) { add(terminal: kind) }
                    }
                }
                if !sources.isEmpty {
                    Menu("Source", systemImage: "dot.radiowaves.left.and.right") {
                        ForEach(sources, id: \.self) { kind in
                            Button(kind.name, systemImage: Self.symbol(source: kind.id)) { add(source: kind) }
                        }
                    }
                }
                Button("Note", systemImage: "note.text") {
                    add(["kind": "note", "title": .string(model.freeTitle("Note")), "config": ["text": ""]])
                }
            }
        } label: {
            Label("Add", systemImage: "plus")
        }
        .disabled(model.doc == nil || model.readOnly || model.deleted)
    }

    private var moreMenu: some View {
        Menu {
            Button("Ask the foreman…", systemImage: "wand.and.stars") { sheet = .foreman }
                .disabled(model.readOnly)
            Button(model.isLive ? "Pause the canvas" : "Go live", systemImage: model.isLive ? "pause.circle" : "play.circle") {
                let live = !model.isLive
                Task { await model.setLive(live) }
            }
            .disabled(model.readOnly)
            Divider()
            Button("Rename…", systemImage: "pencil") {
                renameText = model.doc?.title ?? title
                renaming = .canvas
            }
            .disabled(model.readOnly)
            Button("Settings…", systemImage: "slider.horizontal.3") { sheet = .settings }
            Button("Save as template…", systemImage: "square.on.square") { sheet = .template }
            Divider()
            Button("Delete the canvas…", systemImage: "trash", role: .destructive) { deletingCanvas = true }
                .disabled(model.readOnly)
        } label: {
            Label("More", systemImage: "ellipsis")
        }
        .disabled(model.doc == nil || model.deleted)
    }

    private var harnesses: [HarnessKind] { (model.kinds?.harnesses ?? []).filter { $0.installed != false } }
    private var sources: [SourceKind] { model.kinds?.sources ?? [] }

    static func symbol(harness id: String) -> String {
        switch id {
        case "shell": "terminal"
        case "command": "chevron.left.forwardslash.chevron.right"
        default: "cpu"
        }
    }

    static func symbol(source id: String) -> String {
        switch id {
        case "manual": "hand.tap"
        case "timer": "timer"
        case "files": "doc.badge.clock"
        case "webhook": "link"
        default: "antenna.radiowaves.left.and.right"
        }
    }

    @ViewBuilder
    private func sheetView(_ sheet: Sheet) -> some View {
        switch sheet {
        case let .configure(node):
            NewNodeSheet(model: model, spec: node) { then = $0 }
        case .sessions:
            AdoptSessionSheet(model: model) { then = $0 }
        case .foreman:
            ForemanSheet(model: model) { then = $0 }
        case .settings:
            CanvasSettingsSheet(model: model)
        case .template:
            SaveTemplateSheet(model: model)
        case let .message(message):
            MessageSheet(model: model, message: message)
                .presentationDetents([.medium, .large])
        }
    }

    // MARK: Doing

    private func act(_ action: NodeAction) {
        switch action {
        case let .open(node):
            open(node)
        case let .inspect(node):
            inspected = NodeRef(id: node.id)
        case let .rename(node):
            renameText = node.title
            renaming = .node(node.id)
        case let .approve(node):
            Task { await model.approve(node: node.id) }
        case let .discard(node):
            Task { await model.remove(node.id, discarding: true) }
        case let .remove(node):
            remove(node)
        }
    }

    /// open opens a kou agent's chat, and any other node's details.
    private func open(_ node: CanvasNode) {
        if let route = NodeChat.route(for: node, in: model) {
            openChat(route)
        } else {
            inspected = NodeRef(id: node.id)
        }
    }

    private func openPending() {
        guard let id = then else { return }
        then = nil
        if let node = model.node(id) { open(node) }
    }

    private func add(_ spec: [String: JSONValue]) {
        Task {
            if let made = await model.add(spec), let node = model.node(made) { open(node) }
        }
    }

    private func add(terminal kind: HarnessKind) {
        let title = model.freeTitle(kind.name)
        if let schema = kind.config, !(schema.required ?? []).isEmpty {
            sheet = .configure(NewNode(kind: CanvasNode.terminal, preset: kind.id, plugin: kind.plugin ?? "", title: title, schema: schema))
            return
        }
        var spec: [String: JSONValue] = ["kind": "terminal", "preset": .string(kind.id), "title": .string(title)]
        if let plugin = kind.plugin, !plugin.isEmpty { spec["plugin"] = .string(plugin) }
        add(spec)
    }

    private func add(source kind: SourceKind) {
        let title = model.freeTitle(kind.name)
        if let schema = kind.config, !(schema.required ?? []).isEmpty {
            sheet = .configure(NewNode(kind: CanvasNode.source, preset: kind.id, plugin: kind.plugin ?? "", title: title, schema: schema))
            return
        }
        var spec: [String: JSONValue] = ["kind": "source", "preset": .string(kind.id), "title": .string(title)]
        if let plugin = kind.plugin, !plugin.isEmpty { spec["plugin"] = .string(plugin) }
        add(spec)
    }

    private func remove(_ node: CanvasNode) {
        // An agent that ran has a session to keep, or not.
        if node.kind == CanvasNode.agent, node.session != nil, node.runtime?.briefed == true || node.runtime?.adopted == true {
            removing = node
        } else {
            Task { await model.remove(node.id) }
        }
    }

    private func rename(_ target: Renaming) {
        let text = renameText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        Task {
            switch target {
            case .canvas: await model.renameCanvas(text)
            case let .node(id): await model.rename(id, to: text)
            }
        }
    }

    private func deleteCanvas(keepSessions: Bool) {
        Task {
            if await model.deleteCanvas(keepSessions: keepSessions) { dismiss() }
        }
    }
}

/// NodeChat is a kou agent's chat — its session's own screen, whole: the
/// transcript, the composer, the model, the effort, the queue — with the
/// node it is the agent of a tap away: what waits for it, its settings,
/// its wires.
struct NodeChat: View {
    let model: CanvasModel
    let client: RelayClient
    let route: NodeChatRoute
    var openSession: (SessionRoute) -> Void
    @State private var inspecting = false

    /// The chat of a node: for a kou agent with its session, not one that
    /// waits for the user's approval.
    static func route(for node: CanvasNode, in model: CanvasModel) -> NodeChatRoute? {
        guard node.kind == CanvasNode.agent, !node.isProposed, let session = node.session else { return nil }
        let started = node.runtime?.briefed == true || node.runtime?.adopted == true
        return NodeChatRoute(workspace: model.workspace, canvas: model.id, node: node.id, session: session, fresh: !started,
                             model: node.config?["model"]?.string, title: node.title)
    }

    private var node: CanvasNode? { model.node(route.node) }

    var body: some View {
        SessionScreen(client: client,
                      route: SessionRoute(workspace: route.workspace, id: route.session, fresh: route.fresh,
                                          reserved: true, model: route.model, title: node?.title ?? route.title),
                      canvas: model,
                      open: openSession)
            .safeAreaBar(edge: .top) { banner }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Node", systemImage: "point.3.connected.trianglepath.dotted") { inspecting = true }
                }
            }
            .task { await model.live() }
            .sheet(isPresented: $inspecting) {
                NodeInspector(model: model, nodeID: route.node, inChat: true)
                    .presentationDetents([.medium, .large])
                    .presentationBackgroundInteraction(.enabled(upThrough: .medium))
            }
    }

    @ViewBuilder
    private var banner: some View {
        if let node {
            let awaiting = model.awaiting(for: node.id)
            if awaiting > 0 {
                NodeChatBanner(symbol: "hand.raised.fill",
                               text: awaiting == 1 ? "A message to it waits for your approval." : "\(awaiting) messages to it wait for your approval.",
                               tint: .yellow, action: "Review") { inspecting = true }
            } else if model.status[node.id]?.state == "error", let detail = model.status[node.id]?.detail, !detail.isEmpty {
                NodeChatBanner(symbol: "exclamationmark.triangle.fill", text: detail, tint: .red)
            }
        } else if model.doc != nil, !model.deleted {
            NodeChatBanner(symbol: "square.dashed", text: "It is no longer on the canvas: its session goes on here.", tint: .secondary)
        }
    }
}

/// NodeChatBanner is a word over a node's chat: what waits for the user.
struct NodeChatBanner: View {
    let symbol: String
    let text: String
    var tint: Color = .kouAccent
    var action: String? = nil
    var perform: () -> Void = {}

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(tint)
            Text(text)
                .font(.subheadline)
                .lineLimit(2)
            Spacer(minLength: 4)
            if let action {
                Button(action, action: perform)
                    .font(.subheadline.weight(.semibold))
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassEffect(.regular, in: .rect(cornerRadius: .corner(18)))
        .padding(.horizontal, 12)
    }
}

/// CanvasNoticeSlot shows the canvas's notice while it lasts: a few
/// seconds, longer when it can be undone.
struct CanvasNoticeSlot: View {
    let model: CanvasModel

    var body: some View {
        if let notice = model.notice {
            CanvasNoticeBar(notice: notice) {
                Task { await model.undo(notice) }
            } close: {
                model.notice = nil
            }
            .task(id: notice.id) {
                // What can be undone is for as long as the canvas keeps it.
                try? await Task.sleep(for: .seconds(notice.undo == nil ? 6 : 14))
                guard !Task.isCancelled, model.notice?.id == notice.id else { return }
                model.notice = nil
            }
        }
    }
}

/// CanvasNoticeBar is what the canvas tells: a failure, or what was done,
/// with a way to undo it while the canvas can.
struct CanvasNoticeBar: View {
    let notice: CanvasNotice
    var undo: () -> Void
    var close: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: notice.isError ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                .foregroundStyle(notice.isError ? Color.red : Color.kouAccent)
            Text(notice.text)
                .font(.subheadline)
                .lineLimit(4)
            Spacer(minLength: 4)
            if notice.undo != nil {
                Button("Undo", action: undo)
                    .font(.subheadline.weight(.semibold))
            }
            Button("Dismiss", systemImage: "xmark", action: close)
                .labelStyle(.iconOnly)
                .font(.footnote.weight(.semibold))
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassEffect(.regular, in: .rect(cornerRadius: .corner(18)))
        .padding(.horizontal, 12)
        .transition(.move(edge: .top).combined(with: .opacity))
    }
}
