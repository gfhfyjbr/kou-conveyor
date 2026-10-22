import SwiftUI

/// NodeInspector is a node up close: what it is and does, what waits for
/// it, its settings, what it may do on the canvas and its wires; by its
/// kind, a terminal's screen and keys, a source's log and its button, a
/// note's text. A kou agent's chat is its session's own screen, which the
/// inspector opens.
struct NodeInspector: View {
    let model: CanvasModel
    let nodeID: String
    /// Shown over the node's own chat, which it does not offer again.
    var inChat = false
    /// openChat opens a kou agent's chat.
    var openChat: (CanvasNode) -> Void = { _ in }

    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    /// A terminal's screen or a source's log: nil till read, empty while
    /// its program does not run.
    @State private var screen: String?
    @State private var readError: String?
    @State private var typed = ""
    @State private var fireText: String?
    @State private var noteDraft: String?
    /// The settings changed and not saved yet; .null takes one back to its
    /// default.
    @State private var draft: [String: JSONValue] = [:]
    @State private var sheet: InspectorSheet?
    @State private var renaming = false
    @State private var renameText = ""
    @State private var removing = false
    @State private var working = false
    @State private var lookedForHook = false

    private enum InspectorSheet: Identifiable {
        case edge(CanvasEdge)
        case message(CanvasMessage)
        case models

        var id: String {
            switch self {
            case let .edge(edge): "edge:\(edge.id)"
            case let .message(message): "message:\(message.id)"
            case .models: "models"
            }
        }
    }

    private var node: CanvasNode? { model.node(nodeID) }
    private var editable: Bool { !model.readOnly }
    private var state: String {
        if node?.isProposed == true { return "proposed" }
        return model.status[nodeID]?.state ?? "stopped"
    }

    var body: some View {
        NavigationStack {
            Group {
                if let node {
                    form(node)
                } else {
                    ContentUnavailableView("Gone", systemImage: "square.dashed",
                                           description: Text("The node is no longer on the canvas."))
                }
            }
            .navigationTitle(node == nil ? "Node" : model.title(of: nodeID))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
                if let node, editable {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("Rename", systemImage: "pencil") {
                            renameText = node.title
                            renaming = true
                        }
                    }
                }
            }
        }
        .task(id: nodeID) { await prepare() }
        .task(id: watchedKind) { await watch() }
        .sheet(item: $sheet) { sheet in
            sheetView(sheet)
        }
        .alert("Rename the node", isPresented: $renaming) {
            TextField("Title", text: $renameText)
            Button("Rename") { rename() }
            Button("Cancel", role: .cancel) {}
        }
        .confirmationDialog("Remove «\(model.title(of: nodeID))»?", isPresented: $removing, titleVisibility: .visible) {
            Button("Remove, keep its session", role: .destructive) { remove(keepSession: true) }
            Button("Remove with its session", role: .destructive) { remove(keepSession: false) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("A session kept stays with the workspace's sessions. For a few seconds the node can be put back.")
        }
    }

    // MARK: The form

    private func form(_ node: CanvasNode) -> some View {
        Form {
            header(node)
            if let notice = model.notice, notice.isError {
                Section {
                    Label(notice.text, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(.red)
                        .onTapGesture { model.notice = nil }
                }
            }
            switch node.kind {
            case CanvasNode.agent: lately(node)
            case CanvasNode.terminal: terminal(node)
            case CanvasNode.source: source(node)
            case CanvasNode.note: note(node)
            default: EmptyView()
            }
            waiting
            settings(node)
            access(node)
            wires(node)
            details(node)
            if editable, !node.isProposed {
                Section {
                    Button(role: .destructive) {
                        askRemove(node)
                    } label: {
                        Label("Remove from the canvas", systemImage: "trash")
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func header(_ node: CanvasNode) -> some View {
        Section {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: NodeKindIcon.symbol(node.kind))
                    .font(.title2)
                    .foregroundStyle(node.kind == CanvasNode.agent ? Color.kouAccent : Color.secondary)
                    .frame(width: 32)
                VStack(alignment: .leading, spacing: 4) {
                    Text(Self.kindName(of: node, in: model))
                        .font(.headline)
                    if let line = statusLine(node) {
                        Text(line)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                            .lineLimit(5)
                    }
                }
                Spacer(minLength: 8)
                if node.kind != CanvasNode.note {
                    StateBadge(state: state)
                }
            }
            .padding(.vertical, 2)
            if node.isProposed, editable {
                Button {
                    Task { await model.approve(node: nodeID) }
                } label: {
                    Label("Approve: let it run", systemImage: "checkmark.circle.fill")
                }
                Button(role: .destructive) {
                    Task { if await model.remove(nodeID, discarding: true) { dismiss() } }
                } label: {
                    Label("Discard", systemImage: "xmark.circle")
                }
            }
            if !inChat, NodeChat.route(for: node, in: model) != nil {
                Button {
                    openChat(node)
                } label: {
                    Label("Open the chat", systemImage: "bubble.left.and.bubble.right.fill")
                }
            }
            if editable, node.kind == CanvasNode.agent, state == "busy" {
                Button(role: .destructive) {
                    Task { await model.stop(nodeID) }
                } label: {
                    Label("Stop the run", systemImage: "stop.fill")
                }
            }
        }
    }

    private func statusLine(_ node: CanvasNode) -> String? {
        if node.isProposed {
            return "Proposed by \(Self.creator(of: node, in: model)): nothing runs for it till you approve it."
        }
        let status = model.status[nodeID]
        var parts: [String] = []
        if let activity = status?.activity, !activity.isEmpty { parts.append(activity) }
        if let detail = status?.detail, !detail.isEmpty { parts.append(detail) }
        if let agent = status?.agent, agent.detected == true, let name = agent.title ?? agent.id {
            parts.append("\(name) runs in it")
        }
        if parts.isEmpty, node.kind == CanvasNode.agent, node.runtime?.briefed != true, node.runtime?.adopted != true {
            return "It has not run yet: its first prompt starts it, with the canvas's brief."
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    // MARK: Kinds

    /// What a kou agent did lately, as the canvas tells it; its chat has
    /// the rest.
    @ViewBuilder
    private func lately(_ node: CanvasNode) -> some View {
        if !inChat, let feed = model.feeds[nodeID], !feed.isEmpty {
            Section("Lately") {
                ForEach(feed.suffix(3)) { entry in
                    FeedRow(entry: entry)
                }
            }
        }
    }

    @ViewBuilder
    private func terminal(_ node: CanvasNode) -> some View {
        Section {
            ScreenView(text: screen)
                .listRowInsets(EdgeInsets())
            if editable {
                keys
                HStack(spacing: 10) {
                    TextField("Type a command", text: $typed)
                        .font(.system(.body, design: .monospaced))
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.send)
                        .onSubmit(type)
                    Button("Send", systemImage: "arrow.up.circle.fill", action: type)
                        .labelStyle(.iconOnly)
                        .font(.title2)
                        .buttonStyle(.borderless)
                        .disabled(typed.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        } header: {
            Text("Screen")
        } footer: {
            if let readError {
                Text(readError)
                    .foregroundStyle(.red)
            } else if let agent = model.status[nodeID]?.agent, agent.detected == true {
                Text("What you type goes to \(agent.title ?? agent.id ?? "its agent") as a prompt.")
            }
        }
        if editable {
            Section {
                if Self.runs(state) {
                    Button(role: .destructive) {
                        Task { await model.stop(nodeID) }
                    } label: {
                        Label("Stop what runs (⌃C)", systemImage: "stop.fill")
                    }
                    Button {
                        Task { await model.restart(nodeID) }
                    } label: {
                        Label("Restart", systemImage: "arrow.clockwise")
                    }
                } else {
                    Button {
                        Task { await model.restart(nodeID) }
                    } label: {
                        Label("Start", systemImage: "play.fill")
                    }
                }
                if let agent = node.runtime?.agentSession, !agent.isEmpty {
                    Button {
                        Task { await model.restart(nodeID, resume: true) }
                    } label: {
                        Label("Resume its agent's session", systemImage: "arrow.uturn.forward")
                    }
                }
            } footer: {
                if node.runtime?.agentSession?.isEmpty == false {
                    Text("Restart starts its program anew; resuming starts it again in the conversation its agent had.")
                } else {
                    Text("Restart starts its program anew.")
                }
            }
        }
        output
    }

    private struct TerminalKey: Hashable {
        let label: String
        let name: String
    }

    private static let terminalKeys = [
        TerminalKey(label: "⌃C", name: "C-c"), TerminalKey(label: "esc", name: "escape"), TerminalKey(label: "tab", name: "tab"),
        TerminalKey(label: "↑", name: "up"), TerminalKey(label: "↓", name: "down"), TerminalKey(label: "⏎", name: "enter"),
        TerminalKey(label: "⌃D", name: "C-d"),
    ]

    private var keys: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            GlassEffectContainer(spacing: 6) {
                HStack(spacing: 6) {
                    ForEach(Self.terminalKeys, id: \.name) { key in
                        Button(key.label) { press(key.name) }
                            .font(.system(.footnote, design: .monospaced, weight: .semibold))
                            .buttonStyle(.glass)
                            .controlSize(.small)
                    }
                }
                .padding(.vertical, 2)
            }
        }
    }

    @ViewBuilder
    private func source(_ node: CanvasNode) -> some View {
        if CanvasModel.isManual(node) {
            Section {
                TextField("What it puts out", text: Binding(get: { fireText ?? node.config?["text"]?.string ?? "" },
                                                            set: { fireText = $0 }), axis: .vertical)
                    .lineLimit(2...8)
                Button {
                    fire(node)
                } label: {
                    Label("Fire", systemImage: "bolt.fill")
                }
                .disabled(!editable || !model.isLive || working)
            } header: {
                Text("Fire it")
            } footer: {
                Text(model.isLive ? "It puts the text out along its wires." : "The canvas is paused: it fires once the canvas is live.")
            }
        }
        if node.preset == "webhook" {
            Section {
                if let hook = model.hooks[nodeID] {
                    Text(hook)
                        .font(.system(.footnote, design: .monospaced))
                        .textSelection(.enabled)
                    Button {
                        UIPasteboard.general.string = hook
                        model.notice = CanvasNotice(text: "The address was copied.")
                    } label: {
                        Label("Copy", systemImage: "doc.on.doc")
                    }
                } else {
                    Button {
                        Task { await model.reload() }
                    } label: {
                        Label("Look for its address", systemImage: "arrow.clockwise")
                    }
                }
            } header: {
                Text("Address")
            } footer: {
                Text("A POST to this path of the cockpit fires it: the path is its secret.")
            }
        }
        output
        Section("Log") {
            ScreenView(text: screen, height: 200, empty: "Nothing yet")
                .listRowInsets(EdgeInsets())
        }
    }

    @ViewBuilder
    private var output: some View {
        if let output = model.outputs[nodeID] {
            Section("Last output") {
                VStack(alignment: .leading, spacing: 4) {
                    if let title = output.title, !title.isEmpty {
                        Text(title)
                            .font(.subheadline.weight(.semibold))
                    }
                    Text(output.preview)
                        .font(.subheadline)
                        .lineLimit(10)
                        .textSelection(.enabled)
                    Text(output.at, format: .dateTime.hour().minute().second())
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    @ViewBuilder
    private func note(_ node: CanvasNode) -> some View {
        let text = node.config?["text"]?.string ?? ""
        Section("Note") {
            if noteDraft != nil {
                TextEditor(text: Binding(get: { self.noteDraft ?? "" }, set: { self.noteDraft = $0 }))
                    .frame(minHeight: 180)
                HStack {
                    Button("Cancel") { noteDraft = nil }
                    Spacer()
                    Button("Save") { saveNote() }
                        .fontWeight(.semibold)
                        .disabled(working)
                }
                .buttonStyle(.borderless)
            } else {
                if text.isEmpty {
                    Text("Empty")
                        .foregroundStyle(.secondary)
                } else {
                    MarkdownText(text: text)
                }
                if editable {
                    Button {
                        noteDraft = text
                    } label: {
                        Label("Edit", systemImage: "pencil")
                    }
                }
            }
        }
    }

    // MARK: Around it

    /// The messages that wait for the node: for it to be free, or for the
    /// user's approval.
    @ViewBuilder
    private var waiting: some View {
        let waiting = model.waiting(for: nodeID)
        if !waiting.isEmpty {
            Section {
                ForEach(waiting) { message in
                    Button {
                        sheet = .message(message)
                    } label: {
                        MessageRow(model: model, message: message)
                    }
                    .swipeActions(edge: .leading) {
                        if editable, message.awaitsApproval {
                            Button("Send", systemImage: "paperplane.fill") {
                                Task { await model.approve(message) }
                            }
                            .tint(.green)
                        }
                    }
                    .swipeActions(edge: .trailing) {
                        if editable {
                            Button("Drop", systemImage: "trash") {
                                Task { await model.drop(message) }
                            }
                            .tint(.red)
                        }
                    }
                }
            } header: {
                Text("Waiting for it")
            } footer: {
                if waiting.contains(where: \.awaitsApproval) {
                    Text("Those its wires hold for you go on once you send them, as they are or edited: swipe right, or tap one.")
                }
            }
        }
    }

    @ViewBuilder
    private func settings(_ node: CanvasNode) -> some View {
        if let schema = model.schema(of: node), !(schema.properties ?? [:]).isEmpty {
            Section {
                ForEach(schema.fields) { field in
                    if node.kind == CanvasNode.agent, field.key == "model" {
                        modelRow
                    } else {
                        FieldRow(field: field, value: binding(field.key))
                            .disabled(!editable)
                    }
                }
                if !draft.isEmpty {
                    HStack {
                        Button("Revert") { draft = [:] }
                        Spacer()
                        Button("Save") { saveSettings() }
                            .fontWeight(.semibold)
                            .disabled(working)
                    }
                    .buttonStyle(.borderless)
                }
            } header: {
                Text("Settings")
            }
        } else if model.kinds == nil, node.kind != CanvasNode.note {
            Section("Settings") {
                HStack(spacing: 10) {
                    ProgressView()
                    Text("Reading its settings…")
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private var currentModel: String? {
        if let changed = draft["model"] { return changed.string }
        return node?.config?["model"]?.string
    }

    private var modelRow: some View {
        Button {
            sheet = .models
        } label: {
            LabeledContent {
                Text(app.modelTitle(currentModel, in: model.workspace))
                    .foregroundStyle(Color.secondary)
            } label: {
                Text("Model")
                    .foregroundStyle(Color.primary)
            }
        }
        .disabled(!editable)
    }

    private func binding(_ key: String) -> Binding<JSONValue?> {
        Binding(
            get: {
                if let changed = draft[key] { return changed == .null ? nil : changed }
                let value = model.node(nodeID)?.config?[key]
                return value == .null ? nil : value
            },
            set: { draft[key] = $0 ?? .null }
        )
    }

    @ViewBuilder
    private func access(_ node: CanvasNode) -> some View {
        if node.kind == CanvasNode.agent || node.kind == CanvasNode.terminal {
            let current = node.access.flatMap { $0.isEmpty ? nil : $0 } ?? Self.defaultAccess(of: node)
            Section {
                Picker("Access", selection: Binding(get: { current }, set: { access in
                    Task { await model.setAccess(nodeID, access) }
                })) {
                    ForEach(Self.accesses, id: \.self) { access in
                        Text(access.capitalized).tag(access)
                    }
                }
                .disabled(!editable)
            } header: {
                Text("On the canvas")
            } footer: {
                Text(Self.describe(access: current))
            }
        }
    }

    @ViewBuilder
    private func wires(_ node: CanvasNode) -> some View {
        let edges = model.edges(of: nodeID)
        let targets = model.nodes.filter { other in
            other.id != nodeID && CanvasModel.givesOutput(node) && CanvasModel.takesInput(other)
                && !edges.contains { $0.from.node == nodeID && $0.to.node == other.id }
        }
        let sources = model.nodes.filter { other in
            other.id != nodeID && CanvasModel.takesInput(node) && CanvasModel.givesOutput(other)
                && !edges.contains { $0.from.node == other.id && $0.to.node == nodeID }
        }
        if !edges.isEmpty || (editable && (!targets.isEmpty || !sources.isEmpty)) {
            Section {
                ForEach(edges) { edge in
                    Button {
                        sheet = .edge(edge)
                    } label: {
                        EdgeRow(model: model, edge: edge, from: nodeID)
                    }
                    .swipeActions(edge: .trailing) {
                        if editable {
                            Button("Remove", systemImage: "trash") {
                                Task { await model.remove(edge: edge) }
                            }
                            .tint(.red)
                        }
                    }
                }
                if editable, !targets.isEmpty || !sources.isEmpty {
                    Menu {
                        if !targets.isEmpty {
                            Section("Its output to") {
                                ForEach(targets) { other in
                                    Button(model.title(of: other.id), systemImage: NodeKindIcon.symbol(other.kind)) {
                                        Task { await model.connect(nodeID, to: other.id) }
                                    }
                                }
                            }
                        }
                        if !sources.isEmpty {
                            Section("Into it from") {
                                ForEach(sources) { other in
                                    Button(model.title(of: other.id), systemImage: NodeKindIcon.symbol(other.kind)) {
                                        Task { await model.connect(other.id, to: nodeID) }
                                    }
                                }
                            }
                        }
                    } label: {
                        Label("Wire it…", systemImage: "point.topleft.down.to.point.bottomright.curvepath")
                    }
                }
            } header: {
                Text("Wires")
            } footer: {
                if !edges.isEmpty {
                    Text("Tap a wire to say whether its messages wait for you, and what they say.")
                }
            }
        }
    }

    @ViewBuilder
    private func details(_ node: CanvasNode) -> some View {
        Section("Details") {
            LabeledContent("Made by", value: Self.creator(of: node, in: model))
            if let depth = node.depth, depth > 0 {
                LabeledContent("Depth", value: "\(depth)")
            }
            if let session = node.session ?? node.runtime?.agentSession {
                LabeledContent("Session") {
                    Text(session)
                        .font(.system(.footnote, design: .monospaced))
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }
            }
            if let branch = node.runtime?.branch, !branch.isEmpty {
                LabeledContent("Branch", value: branch)
            }
            if let worktree = node.runtime?.worktree, !worktree.isEmpty {
                LabeledContent("Worktree") {
                    Text(worktree)
                        .font(.system(.footnote, design: .monospaced))
                        .lineLimit(2)
                        .truncationMode(.head)
                        .textSelection(.enabled)
                }
            }
            LabeledContent("ID") {
                Text(node.id)
                    .font(.system(.footnote, design: .monospaced))
                    .textSelection(.enabled)
            }
        }
    }

    @ViewBuilder
    private func sheetView(_ sheet: InspectorSheet) -> some View {
        switch sheet {
        case let .edge(edge):
            EdgeSheet(model: model, edge: edge)
                .presentationDetents([.medium, .large])
        case let .message(message):
            MessageSheet(model: model, message: message)
                .presentationDetents([.medium, .large])
        case .models:
            ModelPicker(workspace: model.workspace, chosen: currentModel, last: nil) { id in
                draft["model"] = id.map(JSONValue.string) ?? .null
            }
            .presentationDetents([.medium, .large])
        }
    }

    // MARK: Doing

    private func prepare() async {
        await model.loadKinds()
        if node?.kind == CanvasNode.agent { _ = await app.loadCatalog(model.workspace) }
        // A webhook made since the canvas was read has its address only in
        // a new reading.
        if let node, node.kind == CanvasNode.source, node.preset == "webhook", model.hooks[nodeID] == nil, !lookedForHook {
            lookedForHook = true
            await model.reload()
        }
    }

    private var watchedKind: String {
        guard let kind = node?.kind, kind == CanvasNode.terminal || kind == CanvasNode.source else { return "" }
        return kind
    }

    /// watch reads a terminal's screen, or a source's log, again and again:
    /// they change without telling.
    private func watch() async {
        let kind = watchedKind
        guard !kind.isEmpty else { return }
        while !Task.isCancelled {
            await readScreen(kind)
            await AppFront.poll(.milliseconds(kind == CanvasNode.terminal ? 1500 : 3000))
        }
    }

    private func readScreen(_ kind: String) async {
        do {
            let read = try await model.read(nodeID, what: kind == CanvasNode.terminal ? "screen" : "tail", lines: 400)
            // Read every second or so, and mostly the same: set only when
            // it changed, so that the sheet is not drawn again for nothing.
            if screen != read.text { screen = read.text }
            if readError != nil { readError = nil }
        } catch is CancellationError {
        } catch RelayError.http(409, _, _) {
            // Not running: nothing to read.
            screen = ""
            readError = nil
        } catch {
            guard !Task.isCancelled else { return }
            if screen == nil { screen = "" }
            readError = error.localizedDescription
        }
    }

    private func press(_ key: String) {
        Task {
            do {
                try await model.send(to: nodeID, text: nil, keys: [key])
                await readScreen(CanvasNode.terminal)
            } catch {
                model.notice = CanvasNotice(text: "The key was not pressed: \(error.localizedDescription)", isError: true)
            }
        }
    }

    private func type() {
        let text = typed
        guard !text.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        typed = ""
        Task {
            do {
                try await model.send(to: nodeID, text: text, submit: true)
                await readScreen(CanvasNode.terminal)
            } catch {
                if typed.isEmpty { typed = text }
                model.notice = CanvasNotice(text: "It was not sent: \(error.localizedDescription)", isError: true)
            }
        }
    }

    private func fire(_ node: CanvasNode) {
        let text = fireText ?? node.config?["text"]?.string ?? ""
        working = true
        Task {
            defer { working = false }
            if await model.fire(nodeID, text: text) {
                model.notice = CanvasNotice(text: "«\(model.title(of: nodeID))» fired.")
            }
        }
    }

    private func saveNote() {
        guard let text = noteDraft else { return }
        working = true
        Task {
            defer { working = false }
            if await model.setConfig(nodeID, ["text": .string(text)]) { noteDraft = nil }
        }
    }

    private func saveSettings() {
        let changes = draft
        working = true
        Task {
            defer { working = false }
            if await model.setConfig(nodeID, changes) { draft = [:] }
        }
    }

    private func rename() {
        let title = renameText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty else { return }
        Task { await model.rename(nodeID, to: title) }
    }

    private func askRemove(_ node: CanvasNode) {
        // An agent that ran has a session to keep, or not.
        if node.kind == CanvasNode.agent, node.session != nil, node.runtime?.briefed == true || node.runtime?.adopted == true {
            removing = true
        } else {
            remove(keepSession: true)
        }
    }

    private func remove(keepSession: Bool) {
        Task {
            if await model.remove(nodeID, keepSession: keepSession) { dismiss() }
        }
    }

    // MARK: Words for a node

    /// Who made a node: the user, or the agent of another.
    static func creator(of node: CanvasNode, in model: CanvasModel) -> String {
        guard let by = node.createdBy, !by.isEmpty, by != "user" else { return "you" }
        guard model.node(by) != nil else { return "an agent" }
        return "«\(model.title(of: by))»"
    }

    /// What a node is: its harness, its source, or what else.
    static func kindName(of node: CanvasNode, in model: CanvasModel) -> String {
        switch node.kind {
        case CanvasNode.agent:
            return node.preset == CanvasNode.foreman ? "Foreman" : "Kou agent"
        case CanvasNode.terminal:
            if let harness = model.harness(of: node) { return harness.name }
            return node.preset.flatMap { $0.isEmpty ? nil : $0.capitalized } ?? "Shell"
        case CanvasNode.source:
            if let source = model.source(of: node) { return source.name }
            return node.preset.flatMap { $0.isEmpty ? nil : "\($0.capitalized) source" } ?? "Source"
        case CanvasNode.note:
            return "Note"
        default:
            return node.kind.capitalized
        }
    }

    /// Whether a node's program runs, as the cockpit's inspector has it.
    static func runs(_ state: String) -> Bool {
        ["starting", "idle", "busy", "waiting"].contains(state)
    }

    static let accesses = ["none", "observe", "talk", "build", "admin"]

    /// The access a node has when none was given: the cockpit's own.
    static func defaultAccess(of node: CanvasNode) -> String {
        if node.kind == CanvasNode.agent { return node.preset == CanvasNode.foreman ? "admin" : "build" }
        let preset = node.preset ?? ""
        return preset.isEmpty || preset == "shell" || preset == "command" ? "talk" : "build"
    }

    static func describe(access: String) -> String {
        switch access {
        case "none": "It does nothing to the canvas."
        case "observe": "It sees the canvas, reads its nodes and waits for them."
        case "talk": "It sees the canvas, messages its nodes and puts out its own."
        case "build": "It also adds, wires and moves nodes, and removes those it made."
        case "admin": "It may do anything: remove any node, change the canvas's settings."
        default: ""
        }
    }
}

/// EdgeRow is a wire of a node: where it goes or comes from, and how.
struct EdgeRow: View {
    let model: CanvasModel
    let edge: CanvasEdge
    /// The node the wire is seen from.
    let from: String

    var body: some View {
        let outgoing = edge.from.node == from
        HStack(spacing: 12) {
            Image(systemName: outgoing ? "arrow.right" : "arrow.left")
                .font(.body.weight(.semibold))
                .foregroundStyle(Color.secondary)
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 3) {
                Text(outgoing ? "To «\(model.title(of: edge.to.node))»" : "From «\(model.title(of: edge.from.node))»")
                    .foregroundStyle(Color.primary)
                    .lineLimit(1)
                Text(Self.describe(edge))
                    .font(.caption)
                    .foregroundStyle(Color.secondary)
            }
            Spacer(minLength: 4)
            switch edge.effectiveMode {
            case CanvasEdge.approve:
                Image(systemName: "hand.raised.fill")
                    .foregroundStyle(Color.yellow)
            case CanvasEdge.off:
                Image(systemName: "pause.circle")
                    .foregroundStyle(Color.secondary)
            default:
                EmptyView()
            }
        }
    }

    static func describe(_ edge: CanvasEdge) -> String {
        var parts: [String] = switch edge.effectiveMode {
        case CanvasEdge.approve: ["Each message waits for you"]
        case CanvasEdge.off: ["Off"]
        default: ["Messages go on their own"]
        }
        if edge.effectiveDeliver == CanvasEdge.now { parts.append("at once") }
        if let template = edge.template, !template.isEmpty { parts.append("templated") }
        return parts.joined(separator: " · ")
    }
}

/// EdgeSheet is a wire up close: whether its messages go on their own,
/// wait for the user or do not go; whether they wait while their target is
/// busy; what they say.
struct EdgeSheet: View {
    let model: CanvasModel
    let edge: CanvasEdge
    @Environment(\.dismiss) private var dismiss
    @State private var mode: String
    @State private var deliver: String
    @State private var header: String
    @State private var template: String
    @State private var working = false

    init(model: CanvasModel, edge: CanvasEdge) {
        self.model = model
        self.edge = edge
        _mode = State(initialValue: edge.effectiveMode)
        _deliver = State(initialValue: edge.effectiveDeliver)
        _header = State(initialValue: Self.header(edge.header))
        _template = State(initialValue: edge.template ?? "")
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledContent("From", value: model.title(of: edge.from.node))
                    LabeledContent("To", value: model.title(of: edge.to.node))
                }
                Section {
                    Picker("Messages", selection: $mode) {
                        Text("Go on their own").tag(CanvasEdge.auto)
                        Text("Wait for you").tag(CanvasEdge.approve)
                        Text("Off").tag(CanvasEdge.off)
                    }
                    Picker("Delivery", selection: $deliver) {
                        Text("When it is free").tag(CanvasEdge.queue)
                        Text("At once").tag(CanvasEdge.now)
                    }
                    Picker("Where it comes from", selection: $header) {
                        Text("As the target wants").tag("default")
                        Text("Said first").tag("on")
                        Text("Not said").tag("off")
                    }
                } footer: {
                    Text(hint)
                }
                Section {
                    TextEditor(text: $template)
                        .font(.system(.callout, design: .monospaced))
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .frame(minHeight: 110)
                } header: {
                    Text("Template")
                } footer: {
                    Text(verbatim: "Empty, the output goes as it is: {{text}}. A template may also use {{title}}, {{data.<path>}}, {{from.title}}, {{from.id}}, {{from.kind}}, {{edge.id}} and {{now}}.")
                }
                if !model.readOnly {
                    Section {
                        Button("Remove the wire", systemImage: "trash", role: .destructive) {
                            Task { if await model.remove(edge: edge) { dismiss() } }
                        }
                    }
                }
            }
            .disabled(model.readOnly)
            .navigationTitle("Wire")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", systemImage: "checkmark") { save() }
                        .disabled(changes.isEmpty || working || model.readOnly)
                }
            }
        }
    }

    private var hint: String {
        let mode = switch self.mode {
        case CanvasEdge.approve: "Each message waits with the canvas's messages till you send it, as it is or edited."
        case CanvasEdge.off: "Nothing goes along it."
        default: "What comes out goes along it on its own."
        }
        let deliver = deliver == CanvasEdge.now ? " It goes at once, even while its target is busy." : " It waits while its target is busy."
        return mode + (self.mode == CanvasEdge.off ? "" : deliver)
    }

    private static func header(_ value: Bool?) -> String {
        value.map { $0 ? "on" : "off" } ?? "default"
    }

    /// What the user changed; a header back to the target's choice goes as
    /// null.
    private var changes: [String: JSONValue] {
        var set: [String: JSONValue] = [:]
        if mode != edge.effectiveMode { set["mode"] = .string(mode) }
        if deliver != edge.effectiveDeliver { set["deliver"] = .string(deliver) }
        if header != Self.header(edge.header) { set["header"] = header == "default" ? .null : .bool(header == "on") }
        if template != (edge.template ?? "") { set["template"] = .string(template) }
        return set
    }

    private func save() {
        let set = changes
        working = true
        Task {
            defer { working = false }
            if await model.update(edge: edge.id, set) { dismiss() }
        }
    }
}

/// FieldRow is a setting of a node's kind, as its schema has it: a choice,
/// a switch, a number, a list or a text. nil is its default.
struct FieldRow: View {
    let field: SchemaField
    @Binding var value: JSONValue?
    /// The text typed: a number or a list half typed stays as it is.
    @State private var text: String

    init(field: SchemaField, value: Binding<JSONValue?>) {
        self.field = field
        _value = value
        _text = State(initialValue: Self.show(value.wrappedValue))
    }

    private var property: SchemaProperty { field.property }
    private var title: String { field.required ? "\(field.title) (required)" : field.title }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            control
            if let description = property.description, !description.isEmpty {
                Text(description)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if let problem {
                Text(problem)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
        .onChange(of: text) { _, new in
            let parsed = parse(new)
            if parsed != value { value = parsed }
        }
        .onChange(of: value) { _, new in
            if parse(text) != new { text = Self.show(new) }
        }
    }

    @ViewBuilder
    private var control: some View {
        if !property.choices.isEmpty {
            Picker(title, selection: Binding(get: { value?.text ?? "" }, set: { value = $0.isEmpty ? nil : .string($0) })) {
                if !field.required || value == nil {
                    Text(fallback).tag("")
                }
                ForEach(property.choices, id: \.self) { choice in
                    Text(choice).tag(choice)
                }
            }
        } else if property.type == "boolean" {
            Toggle(title, isOn: Binding(get: { value?.bool ?? property.defaultValue?.bool ?? false },
                                        set: { value = .bool($0) }))
        } else {
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                TextField(field.title, text: $text, prompt: Text(prompt), axis: .vertical)
                    .lineLimit(1...8)
                    .keyboardType(keyboard)
                    .textInputAutocapitalization(prose ? .sentences : .never)
                    .autocorrectionDisabled(!prose)
            }
        }
    }

    /// The choice that leaves the setting to its default.
    private var fallback: String {
        if field.required { return "Choose" }
        if let value = property.defaultValue?.text, !value.isEmpty { return "Default (\(value))" }
        return "Default"
    }

    private var prompt: String {
        if let value = Self.show(property.defaultValue).nilIfEmpty { return value }
        return property.type == "array" ? "One, another" : (field.required ? "Required" : "Default")
    }

    private var prose: Bool { ["instructions", "text", "description", "prompt"].contains(field.key) }

    private var keyboard: UIKeyboardType {
        switch property.type {
        case "integer": .numberPad
        case "number": .decimalPad
        default: .default
        }
    }

    private var problem: String? {
        guard let pattern = property.pattern, !text.isEmpty, let regex = try? Regex(pattern) else { return nil }
        return text.firstMatch(of: regex) == nil ? "It does not have the form \(pattern)." : nil
    }

    private func parse(_ text: String) -> JSONValue? {
        switch property.type {
        case "number", "integer":
            let trimmed = text.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty { return nil }
            // Half typed, the number stays what it was.
            guard let number = Double(trimmed) else { return value }
            return .number(property.type == "integer" ? number.rounded(.towardZero) : number)
        case "array":
            let items = text.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
            return items.isEmpty ? nil : .array(items.map { .string($0) })
        default:
            return text.isEmpty ? nil : .string(text)
        }
    }

    static func show(_ value: JSONValue?) -> String {
        switch value {
        case let .array(items)?: items.compactMap(\.text).joined(separator: ", ")
        case let value?: value.text ?? ""
        case nil: ""
        }
    }
}

/// ScreenView is a terminal's screen, or a log: light on black.
struct ScreenView: View {
    let text: String?
    var height: CGFloat = 260
    var empty = "Not running"

    /// The text without the blank rows under its last line: a terminal's
    /// screen is as tall as the terminal, and shown from its bottom they
    /// would be all there is to see.
    private var shown: String {
        guard var text else { return "" }
        while let last = text.last, last.isWhitespace { text.removeLast() }
        return text
    }

    // Its long lines wrap: a phone is narrower than a terminal, and a line
    // of a few hundred characters drawn whole is too wide to be drawn at
    // all — the screen stayed black.
    var body: some View {
        let shown = shown
        ScrollView(.vertical) {
            Text(shown)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.green.mix(with: .white, by: 0.35))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(12)
        }
        .defaultScrollAnchor(.bottom, for: .initialOffset)
        .defaultScrollAnchor(.bottom, for: .sizeChanges)
        .frame(height: height)
        .background(Color.black)
        .overlay {
            if text == nil {
                ProgressView()
                    .tint(.white)
            } else if shown.isEmpty {
                Text(empty)
                    .font(.footnote)
                    .foregroundStyle(.white.opacity(0.6))
            }
        }
    }
}

/// FeedRow is a line of a kou agent's feed, as the canvas tells it.
struct FeedRow: View {
    let entry: FeedEntry

    var body: some View {
        switch entry.kind {
        case Entry.user:
            Text(entry.text ?? "")
                .lineLimit(6)
                .foregroundStyle(Color.kouAccentInk)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(Color.kouAccent.gradient, in: .rect(cornerRadius: .corner(16)))
                .frame(maxWidth: .infinity, alignment: .trailing)
                .padding(.leading, 32)
        case Entry.tool:
            HStack(spacing: 8) {
                ToolStateIcon(tool: entry.tool.map { Tool(callID: entry.id, name: $0.name, state: $0.state, exitCode: $0.exitCode) }, live: true)
                    .frame(width: 18)
                Text(entry.tool?.name ?? "tool")
                    .font(.system(.footnote, design: .monospaced, weight: .semibold))
                Text(entry.tool?.summary ?? "")
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.fill.quaternary, in: .rect(cornerRadius: .corner(12)))
        case Entry.reasoning:
            Label(entry.text ?? "Thinking", systemImage: "brain")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .lineLimit(3)
        case Entry.error:
            ErrorBlock(text: entry.text ?? "", detail: nil)
        case Entry.notice:
            NoticeRow(text: entry.text ?? "", detail: nil)
        default:
            MarkdownText(text: entry.text ?? "")
                .lineLimit(8)
        }
    }
}

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
