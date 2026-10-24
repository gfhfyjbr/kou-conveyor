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
                // The node as it is to be: its kind's mark, and the title it
                // is given, which the user may change.
                Section {
                    HStack(spacing: 14) {
                        GlyphTile(symbol: NodeKindIcon.symbol(spec.kind),
                                  tone: spec.kind == CanvasNode.agent ? .kouAccent : .kouFg2, size: 48)
                        VStack(alignment: .leading, spacing: 4) {
                            Eyebrow(spec.kind, color: .kouFg4)
                            TextField("Title", text: $title,
                                      prompt: Text(verbatim: spec.title.isEmpty ? "Title" : spec.title).foregroundStyle(Color.kouFg4))
                                .font(.title3.weight(.bold))
                                .foregroundStyle(Color.kouFg)
                                .submitLabel(.done)
                        }
                    }
                    .padding(.vertical, 4)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 4, trailing: 4))
                }
                if !fields.isEmpty {
                    Section {
                        ForEach(fields) { field in
                            FieldRow(field: field, value: Binding(get: { values[field.key] }, set: { values[field.key] = $0 }))
                        }
                    } header: {
                        Eyebrow("To start")
                    } footer: {
                        FormNote("Its other settings are in its details once it is on the canvas.")
                    }
                    .kouRow()
                }
                if let notice = model.notice, notice.isError {
                    Section {
                        ErrorNote(text: notice.text)
                            .listRowBackground(Color.clear)
                            .listRowInsets(EdgeInsets())
                    }
                }
            }
            .groupedBackground()
            .listSectionSpacing(22)
            .navigationTitle("New \(spec.title.isEmpty ? spec.kind : spec.title)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add", systemImage: "checkmark") { add() }
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
            // As the sessions' own list draws them.
            List(shown) { session in
                Button {
                    adopt(session)
                } label: {
                    SessionRow(session: session)
                }
                .tint(Color.kouFg)
                .disabled(working)
                .listRowBackground(Color.kouGround)
                .listRowSeparatorTint(Color.kouLine)
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(Color.kouGround)
            .overlay {
                if let error {
                    KouEmpty(symbol: "exclamationmark.triangle", title: "Sessions unavailable", message: error, tone: .kouErr)
                } else if sessions == nil {
                    LoadingSpinner()
                } else if shown.isEmpty {
                    if query.isEmpty {
                        KouEmpty(symbol: "text.bubble", title: "No sessions",
                                 message: "Every session of the workspace is on a canvas already.")
                    } else {
                        KouEmpty(symbol: "magnifyingglass", title: "Nothing found",
                                 message: "No session's title has «\(query)» in it.")
                    }
                }
            }
            .searchable(text: $query)
            .navigationTitle("Put a session on the canvas")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
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

    private var foreman: CanvasNode? {
        model.nodes.first { $0.kind == CanvasNode.agent && $0.preset == CanvasNode.foreman }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    head
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 4, trailing: 4))
                }
                Section {
                    TextField("What should be built or done?", text: $text, axis: .vertical)
                        .lineLimit(4...14)
                        .foregroundStyle(Color.kouFg)
                        .focused($focused)
                } header: {
                    Eyebrow("The task")
                } footer: {
                    FormNote(foreman != nil
                        ? "The foreman gets it as a prompt: it adds, wires and runs the nodes it needs."
                        : "The canvas gets a foreman — a kou agent that adds, wires and runs the nodes it needs — with this as its first prompt.")
                }
                .kouRow()
                if let notice = model.notice, notice.isError {
                    Section {
                        ErrorNote(text: notice.text)
                            .listRowBackground(Color.clear)
                            .listRowInsets(EdgeInsets())
                    }
                }
            }
            .groupedBackground()
            .listSectionSpacing(22)
            .navigationTitle("Ask the foreman")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Ask", systemImage: "arrow.up") { ask() }
                        .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || working)
                }
            }
            .onAppear { focused = true }
        }
    }

    /// The foreman the task goes to, and what it does now — or the one the
    /// task makes, dashed till it is on the canvas.
    private var head: some View {
        HStack(spacing: 14) {
            if let foreman {
                NodeTile(node: foreman, state: model.nodeState(foreman), size: 48)
            } else {
                GlyphTile(symbol: NodeKindIcon.symbol(CanvasNode.agent), tone: .kouAccent, size: 48, dashed: true)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: foreman.map { model.title(of: $0.id) } ?? "Foreman")
                    .font(.title3.weight(.bold))
                    .foregroundStyle(Color.kouFg)
                    .lineLimit(1)
                Text("Builds the canvas and runs it")
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg3)
                    .lineLimit(1)
                Group {
                    if let foreman {
                        StateBadge(state: model.nodeState(foreman))
                    } else {
                        StateMark(word: "new", tone: .kouAccent)
                    }
                }
                .padding(.top, 3)
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 4)
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
                Group {
                    Section {
                        TextField("Title", text: $title)
                            .foregroundStyle(Color.kouFg)
                        Toggle(isOn: $live) {
                            Label {
                                Text("Live")
                                    .foregroundStyle(Color.kouFg)
                            } icon: {
                                RowIcon(live ? "dot.radiowaves.left.and.right" : "pause", tone: live ? .kouAccent : .kouFg2)
                            }
                        }
                    } header: {
                        Eyebrow("Canvas")
                    } footer: {
                        FormNote("A live canvas runs its sources and delivers its messages, the cockpit's page open or not; a paused one keeps its messages for later.")
                    }
                    Section {
                        Picker(selection: $spawn) {
                            Text("Run at once").tag("allow")
                            Text("Wait for approval").tag("ask")
                            Text("Not allowed").tag("deny")
                        } label: {
                            Text("Nodes agents make")
                                .foregroundStyle(Color.kouFg)
                        }
                        .tint(Color.kouFg2)
                        number("Nodes at most", $maxNodes, 1...10000)
                        number("Terminals at most", $maxTerminals, 1...64)
                        number("New nodes a minute", $spawnsPerMinute, 1...10000)
                        number("Agents deep", $maxDepth, 1...10000)
                    } header: {
                        Eyebrow("Autonomy")
                    } footer: {
                        FormNote("What the canvas's agents may put on it by themselves.")
                    }
                    Section {
                        number("Messages in a chain", $maxHops, 1...10000)
                        number("Messages a minute per wire", $edgeRate, 1...10000)
                        number("Messages a minute in all", $canvasRate, 1...10000)
                    } header: {
                        Eyebrow("Routing")
                    } footer: {
                        FormNote("What stops agents answering each other forever.")
                    }
                }
                .kouRow()
                if let notice = model.notice, notice.isError {
                    Section {
                        ErrorNote(text: notice.text)
                            .listRowBackground(Color.clear)
                            .listRowInsets(EdgeInsets())
                    }
                }
            }
            .groupedBackground()
            .listSectionSpacing(22)
            .disabled(model.readOnly)
            .navigationTitle("Canvas settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", systemImage: "checkmark") { save() }
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

    /// A limit: its words, and its figure in mono, which rolls as it steps.
    private func number(_ title: String, _ value: Binding<Int>, _ range: ClosedRange<Int>) -> some View {
        Stepper(value: value, in: range) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(title)
                    .foregroundStyle(Color.kouFg)
                Spacer(minLength: 4)
                Text(verbatim: "\(value.wrappedValue)")
                    .font(.system(.body, design: .monospaced, weight: .medium))
                    .foregroundStyle(Color.kouFg2)
                    .contentTransition(.numericText(value: Double(value.wrappedValue)))
                    .animation(.snappy(duration: 0.2), value: value.wrappedValue)
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
                // What is kept: the canvas, and what it is made of.
                Section {
                    HStack(spacing: 14) {
                        GlyphTile(symbol: "square.on.square", tone: .kouFg2, size: 48)
                        VStack(alignment: .leading, spacing: 4) {
                            Text(verbatim: (model.doc?.title).flatMap { $0.isEmpty ? nil : $0 } ?? "Canvas")
                                .font(.title3.weight(.bold))
                                .foregroundStyle(Color.kouFg)
                                .lineLimit(1)
                            Text(verbatim: Self.count(model.nodes.count, "node") + " · " + Self.count(model.edges.count, "wire"))
                                .font(.kouMeta)
                                .foregroundStyle(Color.kouFg3)
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.vertical, 4)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 4, trailing: 4))
                }
                Group {
                    Section {
                        TextField("Name", text: $name)
                            .foregroundStyle(Color.kouFg)
                    } header: {
                        Eyebrow("Name")
                    }
                    Section {
                        TextField("What it is for", text: $description, axis: .vertical)
                            .lineLimit(2...6)
                            .foregroundStyle(Color.kouFg)
                    } header: {
                        Eyebrow("What it is for")
                    } footer: {
                        FormNote("A new canvas can start from it: its nodes, their settings and wires, without their sessions and messages.")
                    }
                }
                .kouRow()
                if let notice = model.notice, notice.isError {
                    Section {
                        ErrorNote(text: notice.text)
                            .listRowBackground(Color.clear)
                            .listRowInsets(EdgeInsets())
                    }
                }
            }
            .groupedBackground()
            .listSectionSpacing(22)
            .navigationTitle("Save as template")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", systemImage: "checkmark") { save() }
                        .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || working)
                }
            }
            .onAppear {
                if name.isEmpty { name = model.doc?.title ?? "" }
            }
        }
    }

    private static func count(_ count: Int, _ word: String) -> String {
        "\(count) \(word)\(count == 1 ? "" : "s")"
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
                    head
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 4, trailing: 4))
                }
                Group {
                    Section {
                        if let title = current.title, !title.isEmpty {
                            Text(verbatim: title)
                                .font(.headline)
                                .foregroundStyle(Color.kouFg)
                        }
                        // In mono, as the cockpit sets what goes along a wire.
                        if editing {
                            TextEditor(text: $draft)
                                .font(.system(.callout, design: .monospaced))
                                .foregroundStyle(Color.kouFg)
                                .scrollContentBackground(.hidden)
                                .frame(minHeight: 220)
                        } else {
                            Text(text)
                                .font(.system(.callout, design: .monospaced))
                                .foregroundStyle(Color.kouFg)
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            if current.truncated == true, whole == nil {
                                ProgressView()
                                    .tint(Color.kouFg3)
                            }
                        }
                    } header: {
                        ListHead(editing ? "Editing" : "Text") {
                            if editing {
                                Tag("Not sent", tone: .kouWarn)
                            }
                        }
                    }
                    if current.isWaiting, !model.readOnly {
                        Section {
                            if current.awaitsApproval {
                                Button {
                                    approve()
                                } label: {
                                    ActionLabel(editing ? "Send as edited" : "Send", symbol: "paperplane.fill", tone: .kouAccent)
                                }
                                .disabled(working || editing && draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                                if !editing {
                                    Button {
                                        draft = (whole ?? current).text ?? ""
                                        editing = true
                                    } label: {
                                        ActionLabel("Edit, then send", symbol: "pencil")
                                    }
                                    .disabled(working || current.truncated == true && whole == nil)
                                }
                            }
                            Button(role: .destructive) {
                                drop()
                            } label: {
                                ActionLabel("Drop", symbol: "trash", tone: .kouErr, ink: .kouErr)
                            }
                            .disabled(working)
                        } footer: {
                            FormNote(verbatim: current.awaitsApproval
                                ? "Its wire asks before each message goes."
                                : "It goes once «\(name(current.to?.node))» is free\(model.isLive ? "" : " and the canvas is live").")
                        }
                    }
                }
                .kouRow()
                if let error {
                    Section {
                        ErrorNote(text: error)
                            .listRowBackground(Color.clear)
                            .listRowInsets(EdgeInsets())
                    }
                }
            }
            .groupedBackground()
            .listSectionSpacing(22)
            .navigationTitle("Message")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(editing ? "Cancel" : "Close", systemImage: "xmark") {
                        if editing {
                            editing = false
                        } else {
                            dismiss()
                        }
                    }
                    .tint(Color.kouFg)
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

    /// Where it goes along, as its wire is drawn — dashed while it waits —,
    /// how it stands and when it came.
    private var head: some View {
        let state = current.state ?? ""
        let tone = MessageRow.color(state)
        return VStack(alignment: .leading, spacing: 14) {
            WireRoute(model: model, from: current.from?.node, to: current.to?.node, tone: tone,
                      dash: current.awaitsApproval ? [4, 4] : state == CanvasMessage.pending ? [2, 4]
                          : state == CanvasMessage.dropped ? [1, 5] : [])
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                if current.state != nil {
                    HStack(alignment: .firstTextBaseline, spacing: 7) {
                        Rectangle()
                            .fill(tone)
                            .frame(width: 6, height: 6)
                            .alignmentGuide(.firstTextBaseline) { $0[.bottom] }
                        Text(verbatim: MessageRow.describe(current))
                            .font(.kouMeta)
                            .foregroundStyle(tone)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 8)
                if let at = current.at {
                    Text(at, format: .dateTime.hour().minute().second())
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg4)
                }
            }
            if let hops = current.hops, hops > 1 {
                Text(verbatim: "In a chain of \(hops) messages")
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg3)
            }
        }
        .padding(.vertical, 4)
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

/// WireRoute is a wire as the board draws it, from its source down to its
/// target: in the warning's dashes while what goes along it waits for the
/// user, sparse while off, plain while it goes on its own. An end that is
/// no node is the user.
struct WireRoute: View {
    let model: CanvasModel
    let from: String?
    let to: String?
    var tone: Color = .kouFg3
    var dash: [CGFloat] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            end("From", from)
            Path { path in
                path.move(to: CGPoint(x: 1, y: 0))
                path.addLine(to: CGPoint(x: 1, y: 22))
            }
            .stroke(tone, style: StrokeStyle(lineWidth: 2, lineCap: .round, dash: dash))
            .frame(width: 2, height: 22)
            // Under the middle of the tiles.
            .padding(.leading, 16)
            .padding(.vertical, 5)
            end("To", to)
        }
        .padding(.vertical, 4)
    }

    private func end(_ label: String, _ id: String?) -> some View {
        let node = id.flatMap { $0.isEmpty ? nil : model.node($0) }
        let you = id?.isEmpty ?? true
        return HStack(spacing: 12) {
            if let node {
                NodeTile(node: node, state: model.nodeState(node))
            } else {
                GlyphTile(symbol: you ? "person.fill" : "square.dashed", tone: .kouFg3)
            }
            VStack(alignment: .leading, spacing: 2) {
                Eyebrow(label, color: .kouFg4)
                Text(verbatim: you ? "You" : model.title(of: id ?? ""))
                    .font(.body.weight(.semibold))
                    .foregroundStyle(Color.kouFg)
                    .lineLimit(1)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// GlyphTile is a mark on a node's tile, for what is no node of the canvas:
/// one yet to be made — dashed —, the user at a wire's end, a template.
struct GlyphTile: View {
    let symbol: String
    var tone: Color = .kouFg2
    var size: CGFloat = 34
    var dashed = false

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: .corner(size * 0.26), style: .continuous)
        Image(systemName: symbol)
            .font(.system(size: size * 0.42, weight: .medium))
            .foregroundStyle(tone)
            .frame(width: size, height: size)
            .background(Color.kouPanel, in: shape)
            .overlay {
                shape.strokeBorder(dashed ? tone.opacity(0.6) : Color.kouLine,
                                   style: StrokeStyle(lineWidth: 1, dash: dashed ? [3, 2.5] : []))
            }
            .accessibilityHidden(true)
    }
}

extension CanvasModel {
    /// nodeState is a node's state as its tile shows it: proposed till the
    /// user approves it, stopped till the canvas says otherwise.
    func nodeState(_ node: CanvasNode) -> String {
        node.isProposed ? "proposed" : status[node.id]?.state ?? "stopped"
    }
}
