import SwiftUI

/// NewNodeSheet adds a node whose kind needs settings to start: a command's
/// command, the files a source watches. Its other settings wait in its
/// details.
struct NewNodeSheet: View {
    let model: CanvasModel
    let spec: NewNode
    var added: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var title: String
    @State private var values: [String: JSONValue] = [:]
    @State private var working = false

    init(model: CanvasModel, spec: NewNode, added: @escaping (String) -> Void) {
        self.model = model
        self.spec = spec
        self.added = added
        _title = State(initialValue: spec.title)
    }

    private var fields: [SchemaField] { spec.schema.fields.filter(\.required) }

    private var complete: Bool {
        fields.allSatisfy { field in
            guard let value = values[field.key] else { return false }
            return !(value.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || value.bool != nil
        }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Title", text: $title)
                }
                Section {
                    ForEach(fields) { field in
                        FieldRow(field: field, value: Binding(get: { values[field.key] }, set: { values[field.key] = $0 }))
                    }
                } footer: {
                    Text("Its other settings are in its details once it is on the canvas.")
                }
                if let notice = model.notice, notice.isError {
                    Section {
                        Text(notice.text)
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("New \(spec.title.isEmpty ? spec.kind : spec.title)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") { add() }
                        .disabled(!complete || working)
                }
            }
        }
    }

    private func add() {
        var node: [String: JSONValue] = [
            "kind": .string(spec.kind),
            "preset": .string(spec.preset),
            "title": .string(title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? spec.title : title),
            "config": .object(values),
        ]
        if !spec.plugin.isEmpty { node["plugin"] = .string(spec.plugin) }
        working = true
        model.notice = nil
        Task {
            defer { working = false }
            if let made = await model.add(node) {
                added(made)
                dismiss()
            }
        }
    }
}

/// AdoptSessionSheet puts a session of the workspace on the canvas: it goes
/// on as it was, as a kou agent's, with the canvas's tools from its next
/// run.
struct AdoptSessionSheet: View {
    let model: CanvasModel
    var added: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var sessions: [SessionSummary]?
    @State private var error: String?
    @State private var query = ""
    @State private var working = false

    private var shown: [SessionSummary] {
        let words = query.lowercased().split(separator: " ")
        return (sessions ?? []).filter { session in
            words.allSatisfy { session.title.lowercased().contains($0) }
        }
    }

    var body: some View {
        NavigationStack {
            List(shown) { session in
                Button {
                    adopt(session)
                } label: {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(session.title.isEmpty ? "Untitled session" : session.title)
                            .foregroundStyle(Color.primary)
                            .lineLimit(2)
                        HStack(spacing: 8) {
                            if session.lastActive > .distantPast {
                                Text(session.lastActive, format: .relative(presentation: .named))
                            }
                            if session.running {
                                Text("running")
                                    .foregroundStyle(Color.kouAccent)
                            }
                        }
                        .font(.caption)
                        .foregroundStyle(Color.secondary)
                    }
                }
                .disabled(working)
            }
            .overlay {
                if let error {
                    ContentUnavailableView("Sessions unavailable", systemImage: "exclamationmark.triangle", description: Text(error))
                } else if sessions == nil {
                    LoadingSpinner()
                } else if shown.isEmpty {
                    if query.isEmpty {
                        ContentUnavailableView("No sessions", systemImage: "text.bubble",
                                               description: Text("Every session of the workspace is on a canvas already."))
                    } else {
                        ContentUnavailableView.search(text: query)
                    }
                }
            }
            .searchable(text: $query)
            .navigationTitle("Put a session on the canvas")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .task {
                do {
                    sessions = try await model.freeSessions()
                } catch is CancellationError {
                } catch {
                    self.error = error.localizedDescription
                }
            }
        }
    }

    private func adopt(_ session: SessionSummary) {
        let base = session.title.isEmpty ? "Agent" : String(session.title.prefix(60))
        working = true
        Task {
            defer { working = false }
            if let made = await model.add(["kind": "agent", "session": .string(session.id), "title": .string(model.freeTitle(base))]) {
                added(made)
                dismiss()
            } else if let notice = model.notice, notice.isError {
                error = notice.text
            }
        }
    }
}

/// ForemanSheet gives the canvas's foreman a task: the kou agent that
/// builds the canvas and runs it. A canvas without one gets one.
struct ForemanSheet: View {
    let model: CanvasModel
    var asked: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var working = false
    @FocusState private var focused: Bool

    private var hasForeman: Bool {
        model.nodes.contains { $0.kind == CanvasNode.agent && $0.preset == CanvasNode.foreman }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("What should be built or done?", text: $text, axis: .vertical)
                        .lineLimit(4...14)
                        .focused($focused)
                } footer: {
                    Text(hasForeman
                        ? "The foreman gets it as a prompt: it adds, wires and runs the nodes it needs."
                        : "The canvas gets a foreman — a kou agent that adds, wires and runs the nodes it needs — with this as its first prompt.")
                }
                if let notice = model.notice, notice.isError {
                    Section {
                        Text(notice.text)
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Ask the foreman")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Ask") { ask() }
                        .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || working)
                }
            }
            .onAppear { focused = true }
        }
    }

    private func ask() {
        let task = text.trimmingCharacters(in: .whitespacesAndNewlines)
        working = true
        model.notice = nil
        Task {
            defer { working = false }
            if let node = await model.askForeman(task) {
                asked(node)
                dismiss()
            }
        }
    }
}

/// CanvasSettingsSheet is how far the canvas's agents go: the nodes they
/// may make, and the messages they may send.
struct CanvasSettingsSheet: View {
    let model: CanvasModel
    @Environment(\.dismiss) private var dismiss
    @State private var title = ""
    @State private var live = false
    @State private var spawn = "allow"
    @State private var maxNodes = 48
    @State private var maxTerminals = 16
    @State private var spawnsPerMinute = 6
    @State private var maxDepth = 3
    @State private var maxHops = 32
    @State private var edgeRate = 30
    @State private var canvasRate = 120
    @State private var read = false
    @State private var working = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Title", text: $title)
                    Toggle("Live", isOn: $live)
                } footer: {
                    Text("A live canvas runs its sources and delivers its messages, the cockpit's page open or not; a paused one keeps its messages for later.")
                }
                Section {
                    Picker("Nodes agents make", selection: $spawn) {
                        Text("Run at once").tag("allow")
                        Text("Wait for approval").tag("ask")
                        Text("Not allowed").tag("deny")
                    }
                    number("Nodes at most", $maxNodes, 1...10000)
                    number("Terminals at most", $maxTerminals, 1...64)
                    number("New nodes a minute", $spawnsPerMinute, 1...10000)
                    number("Agents deep", $maxDepth, 1...10000)
                } header: {
                    Text("Autonomy")
                } footer: {
                    Text("What the canvas's agents may put on it by themselves.")
                }
                Section {
                    number("Messages in a chain", $maxHops, 1...10000)
                    number("Messages a minute per wire", $edgeRate, 1...10000)
                    number("Messages a minute in all", $canvasRate, 1...10000)
                } header: {
                    Text("Routing")
                } footer: {
                    Text("What stops agents answering each other forever.")
                }
                if let notice = model.notice, notice.isError {
                    Section {
                        Text(notice.text)
                            .foregroundStyle(.red)
                    }
                }
            }
            .disabled(model.readOnly)
            .navigationTitle("Canvas settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") { save() }
                        .disabled(working || model.readOnly)
                }
            }
            .onAppear {
                guard !read else { return }
                read = true
                load()
            }
        }
    }

    private func number(_ title: String, _ value: Binding<Int>, _ range: ClosedRange<Int>) -> some View {
        Stepper(value: value, in: range) {
            LabeledContent(title) {
                Text("\(value.wrappedValue)")
                    .monospacedDigit()
            }
        }
    }

    private func load() {
        let doc = model.doc
        title = doc?.title ?? ""
        live = doc?.live ?? false
        let autonomy = doc?.settings?["autonomy"]
        let routing = doc?.settings?["routing"]
        spawn = autonomy?["spawn"]?.string ?? "allow"
        maxNodes = Self.int(autonomy?["max_nodes"], 48)
        maxTerminals = Self.int(autonomy?["max_terminals"], 16)
        spawnsPerMinute = Self.int(autonomy?["spawns_per_minute"], 6)
        maxDepth = Self.int(autonomy?["max_depth"], 3)
        maxHops = Self.int(routing?["max_hops"], 32)
        edgeRate = Self.int(routing?["edge_rate_per_minute"], 30)
        canvasRate = Self.int(routing?["canvas_rate_per_minute"], 120)
    }

    private static func int(_ value: JSONValue?, _ fallback: Int) -> Int {
        guard let number = value?.number, number >= 1 else { return fallback }
        return Int(number)
    }

    private func save() {
        // The settings go back whole, with what this app does not know of.
        var settings = model.doc?.settings?.object ?? [:]
        var autonomy = settings["autonomy"]?.object ?? [:]
        autonomy["spawn"] = .string(spawn)
        autonomy["max_nodes"] = .number(Double(maxNodes))
        autonomy["max_terminals"] = .number(Double(maxTerminals))
        autonomy["spawns_per_minute"] = .number(Double(spawnsPerMinute))
        autonomy["max_depth"] = .number(Double(maxDepth))
        settings["autonomy"] = .object(autonomy)
        var routing = settings["routing"]?.object ?? [:]
        routing["max_hops"] = .number(Double(maxHops))
        routing["edge_rate_per_minute"] = .number(Double(edgeRate))
        routing["canvas_rate_per_minute"] = .number(Double(canvasRate))
        settings["routing"] = .object(routing)
        var set: [String: JSONValue] = ["settings": .object(settings)]
        let name = title.trimmingCharacters(in: .whitespacesAndNewlines)
        if !name.isEmpty, name != model.doc?.title { set["title"] = .string(name) }
        if live != model.isLive { set["live"] = .bool(live) }
        working = true
        model.notice = nil
        Task {
            defer { working = false }
            if await model.updateCanvas(set, failure: "The settings were not saved") { dismiss() }
        }
    }
}

/// SaveTemplateSheet keeps the canvas as a template: its nodes, settings
/// and wires, which new canvases start from.
struct SaveTemplateSheet: View {
    let model: CanvasModel
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var description = ""
    @State private var working = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name", text: $name)
                    TextField("What it is for", text: $description, axis: .vertical)
                        .lineLimit(2...6)
                } footer: {
                    Text("A new canvas can start from it: its nodes, their settings and wires, without their sessions and messages.")
                }
                if let notice = model.notice, notice.isError {
                    Section {
                        Text(notice.text)
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Save as template")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") { save() }
                        .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || working)
                }
            }
            .onAppear {
                if name.isEmpty { name = model.doc?.title ?? "" }
            }
        }
    }

    private func save() {
        working = true
        model.notice = nil
        Task {
            defer { working = false }
            let saved = await model.saveTemplate(name: name.trimmingCharacters(in: .whitespacesAndNewlines),
                                                 description: description.trimmingCharacters(in: .whitespacesAndNewlines))
            if saved {
                model.notice = CanvasNotice(text: "Saved as the template «\(name)».")
                dismiss()
            }
        }
    }
}

/// MessageSheet is a message whole: where it comes from and goes, why it
/// waits; one that waits for approval is sent as it is or edited, or
/// dropped.
struct MessageSheet: View {
    let model: CanvasModel
    let message: CanvasMessage
    @Environment(\.dismiss) private var dismiss
    @State private var whole: CanvasMessage?
    @State private var editing = false
    @State private var draft = ""
    @State private var working = false
    @State private var error: String?

    /// The message as the canvas has it now.
    private var current: CanvasMessage {
        model.queue.first { $0.id == message.id } ?? model.messages.first { $0.id == message.id } ?? message
    }

    private var text: String { (whole ?? current).content }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledContent("From", value: name(current.from?.node))
                    LabeledContent("To", value: name(current.to?.node))
                    if let at = current.at {
                        LabeledContent("At") {
                            Text(at, format: .dateTime.hour().minute().second())
                        }
                    }
                    if current.state != nil {
                        LabeledContent("State") {
                            Text(MessageRow.describe(current))
                                .foregroundStyle(MessageRow.color(current.state ?? ""))
                        }
                    }
                    if let hops = current.hops, hops > 1 {
                        LabeledContent("In a chain of", value: "\(hops) messages")
                    }
                }
                Section {
                    if editing {
                        TextEditor(text: $draft)
                            .frame(minHeight: 220)
                    } else {
                        Text(text)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if current.truncated == true, whole == nil {
                            ProgressView()
                        }
                    }
                } header: {
                    Text(current.title.flatMap { $0.isEmpty ? nil : $0 } ?? "Text")
                }
                if current.isWaiting, !model.readOnly {
                    Section {
                        if current.awaitsApproval {
                            Button(editing ? "Send as edited" : "Send", systemImage: "paperplane.fill") { approve() }
                                .disabled(working || editing && draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                            if !editing {
                                Button("Edit, then send", systemImage: "pencil") {
                                    draft = (whole ?? current).text ?? ""
                                    editing = true
                                }
                                .disabled(working || current.truncated == true && whole == nil)
                            }
                        }
                        Button("Drop", systemImage: "trash", role: .destructive) { drop() }
                            .disabled(working)
                    } footer: {
                        Text(current.awaitsApproval
                            ? "Its wire asks before each message goes."
                            : "It goes once «\(name(current.to?.node))» is free\(model.isLive ? "" : " and the canvas is live").")
                    }
                }
                if let error {
                    Section {
                        Text(error)
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Message")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(editing ? "Cancel" : "Close") {
                        if editing {
                            editing = false
                        } else {
                            dismiss()
                        }
                    }
                }
            }
            .task {
                do {
                    whole = try await model.whole(message)
                } catch is CancellationError {
                } catch {
                    self.error = error.localizedDescription
                }
            }
        }
    }

    private func name(_ node: String?) -> String {
        guard let node, !node.isEmpty else { return "You" }
        return model.node(node)?.title ?? node
    }

    private func approve() {
        let edited = editing ? draft : nil
        working = true
        model.notice = nil
        Task {
            defer { working = false }
            if await model.approve(current, text: edited) {
                dismiss()
            } else {
                error = model.notice?.text
            }
        }
    }

    private func drop() {
        working = true
        model.notice = nil
        Task {
            defer { working = false }
            if await model.drop(current) {
                dismiss()
            } else {
                error = model.notice?.text
            }
        }
    }
}
