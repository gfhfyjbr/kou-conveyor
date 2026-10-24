import SwiftUI

struct CanvasRoute: Hashable {
    let workspace: String
    let id: String
    let title: String
}

/// CanvasesScreen is the Canvases tab: the workspace's canvases, a canvas
/// opened from them, and the chats of its agents.
struct CanvasesScreen: View {
    @Environment(AppModel.self) private var app
    @State private var path = NavigationPath()

    var body: some View {
        NavigationStack(path: $path) {
            CockpitGate { client, workspace in
                CanvasList(client: client, workspace: workspace) { path.append($0) }
                    .id(workspace.id)
            }
            .navigationTitle("Canvases")
            .navigationDestination(for: CanvasRoute.self) { route in
                if let client = app.client {
                    CanvasScreen(client: client, route: route,
                                 openChat: { path.append($0) },
                                 openSession: { path.append($0) })
                }
            }
            .navigationDestination(for: SessionRoute.self) { route in
                if let client = app.client {
                    SessionScreen(client: client, route: route) { path.append($0) }
                }
            }
        }
        // A canvas takes the whole screen, and the chats opened from it.
        .hidesTabBar(when: !path.isEmpty)
        // What was open is another workspace's.
        .onChange(of: app.workspace?.id) {
            path = NavigationPath()
        }
    }
}

struct CanvasList: View {
    @Environment(AppModel.self) private var app
    @State private var model: CanvasesModel
    @State private var creating = false
    @State private var renaming: CanvasSummary?
    @State private var newTitle = ""
    @State private var deleting: CanvasSummary?
    @State private var failure: String?
    let workspace: Workspace
    var open: (CanvasRoute) -> Void

    init(client: RelayClient, workspace: Workspace, open: @escaping (CanvasRoute) -> Void) {
        _model = State(initialValue: CanvasesModel(client: client, workspace: workspace.id))
        self.workspace = workspace
        self.open = open
    }

    var body: some View {
        // The live canvases — the ones that pass what they make on — apart
        // from the paused, as the sessions list keeps the pinned apart.
        let live = model.canvases.filter { $0.live == true }
        let paused = model.canvases.filter { $0.live != true }
        List {
            if !live.isEmpty {
                Section {
                    ForEach(live) { row($0) }
                } header: {
                    SectionLabel("Live", count: live.count, color: .kouAccent)
                        .kouHeader()
                }
            }
            if !paused.isEmpty {
                Section {
                    ForEach(paused) { row($0) }
                } header: {
                    SectionLabel("Paused", count: paused.count)
                        .kouHeader(top: live.isEmpty ? 14 : 22)
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color.kouGround)
        .animation(.snappy, value: model.canvases.map { "\($0.id)\($0.live == true)" })
        .overlay {
            if !model.loaded {
                LoadingSpinner()
            } else if let error = model.error, model.canvases.isEmpty {
                KouEmpty(symbol: "point.3.connected.trianglepath.dotted", title: "Canvases unavailable",
                         message: error, tone: .kouErr)
            } else if model.canvases.isEmpty {
                KouEmpty(symbol: "point.3.connected.trianglepath.dotted", title: "No canvases",
                         message: "A canvas wires terminals, agents and sources together: what one puts out, the next is given.") {
                    Button("New canvas", systemImage: "plus") { creating = true }
                        .buttonStyle(.glassProminent)
                        .disabled(model.unavailable)
                }
            }
        }
        .refreshable { _ = await model.load() }
        .task { await model.poll(noticed: app.noticed) }
        .navigationSubtitle(workspace.name)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                WorkspaceMenu()
            }
            // Not while the empty list shows its own button: one call to
            // make a canvas, not two in the accent.
            if !(model.loaded && model.error == nil && model.canvases.isEmpty) {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("New canvas", systemImage: "plus") { creating = true }
                        .buttonStyle(.glassProminent)
                        .disabled(model.unavailable)
                }
            }
        }
        .sensoryFeedback(.selection, trigger: live.count)
        .sheet(isPresented: $creating) {
            NewCanvasSheet(model: model) { made in
                open(CanvasRoute(workspace: workspace.id, id: made.id, title: made.title))
            }
        }
        .alert("Rename the canvas", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Title", text: $newTitle)
            Button("Rename") {
                if let canvas = renaming { rename(canvas, to: newTitle) }
            }
            Button("Cancel", role: .cancel) {}
        }
        .confirmationDialog(deleting.map { "Delete «\($0.title)»?" } ?? "Delete the canvas?",
                            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
                            titleVisibility: .visible, presenting: deleting) { canvas in
            Button("Delete, keep its agents' sessions", role: .destructive) { delete(canvas, keepSessions: true) }
            Button("Delete with its agents' sessions", role: .destructive) { delete(canvas, keepSessions: false) }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("Its terminals stop and its messages go. Sessions kept stay in the sessions list.")
        }
        .alert("It did not work", isPresented: Binding(get: { failure != nil }, set: { if !$0 { failure = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(failure ?? "")
        }
    }

    private func row(_ canvas: CanvasSummary) -> some View {
        NavigationLink(value: CanvasRoute(workspace: workspace.id, id: canvas.id, title: canvas.title)) {
            CanvasRow(canvas: canvas)
        }
        .navigationLinkIndicatorVisibility(.hidden)
        .listRowBackground(Color.kouGround)
        .listRowSeparatorTint(Color.kouLine)
        .listRowInsets(EdgeInsets(top: 12, leading: 16, bottom: 12, trailing: 18))
        .contextMenu { menu(for: canvas) }
        .swipeActions(edge: .trailing) {
            Button("Delete", systemImage: "trash") { deleting = canvas }
                .tint(Color.kouErr)
        }
        .swipeActions(edge: .leading) {
            Button(canvas.live == true ? "Pause" : "Go live", systemImage: canvas.live == true ? "pause.fill" : "play.fill") {
                setLive(canvas, canvas.live != true)
            }
            .tint(canvas.live == true ? .gray : Color.kouAccent)
        }
    }

    @ViewBuilder
    private func menu(for canvas: CanvasSummary) -> some View {
        Button("Rename…", systemImage: "pencil") {
            newTitle = canvas.title
            renaming = canvas
        }
        Button(canvas.live == true ? "Pause" : "Go live", systemImage: canvas.live == true ? "pause.circle" : "play.circle") {
            setLive(canvas, canvas.live != true)
        }
        Divider()
        Button("Delete…", systemImage: "trash", role: .destructive) { deleting = canvas }
    }

    private func rename(_ canvas: CanvasSummary, to title: String) {
        let title = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty, title != canvas.title else { return }
        perform { try await model.rename(canvas, to: title) }
    }

    private func setLive(_ canvas: CanvasSummary, _ live: Bool) {
        perform { try await model.setLive(canvas, live) }
    }

    private func delete(_ canvas: CanvasSummary, keepSessions: Bool) {
        perform { try await model.delete(canvas, keepSessions: keepSessions) }
    }

    private func perform(_ work: @escaping @MainActor () async throws -> Void) {
        Task {
            do {
                try await work()
            } catch is CancellationError {
            } catch {
                app.noticed(error)
                failure = error.localizedDescription
            }
        }
    }
}

/// NewCanvasSheet makes a canvas: empty, or from a template.
struct NewCanvasSheet: View {
    let model: CanvasesModel
    var made: (CanvasSummary) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var title = ""
    @State private var template = "empty"
    @State private var working = false
    @State private var error: String?
    /// The templates are being asked for; once they are not, a canvas
    /// without them starts empty.
    @State private var loading = true
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            Form {
                // The canvas as it is to be: its mark, the title it is given.
                Section {
                    HStack(spacing: 14) {
                        GlyphTile(symbol: "point.3.filled.connected.trianglepath.dotted", tone: .kouFg2, size: 48)
                        VStack(alignment: .leading, spacing: 4) {
                            Eyebrow("Canvas", color: .kouFg4)
                            TextField("Title", text: $title, prompt: Text(verbatim: defaultName).foregroundStyle(Color.kouFg4))
                                .font(.title3.weight(.bold))
                                .foregroundStyle(Color.kouFg)
                                .focused($focused)
                                .submitLabel(.done)
                        }
                    }
                    .padding(.vertical, 4)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 4, trailing: 4))
                } footer: {
                    FormNote(verbatim: "Left empty, it is called «\(defaultName)».")
                }
                Section {
                    if model.templates.isEmpty {
                        choice("Empty", chosen: true, loading: loading)
                    }
                    ForEach(model.templates) { item in
                        Button {
                            template = item.id
                        } label: {
                            choice(item.name, detail: item.description, facts: Self.facts(of: item), chosen: template == item.id)
                        }
                        .tint(Color.kouFg)
                    }
                } header: {
                    Eyebrow("Start from")
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
            .sensoryFeedback(.selection, trigger: template)
            .navigationTitle("New canvas")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Create", systemImage: "checkmark") { create() }
                        .disabled(working)
                }
            }
            .task {
                focused = true
                defer { loading = false }
                do {
                    try await model.loadTemplates()
                    if !model.templates.contains(where: { $0.id == template }), let first = model.templates.first {
                        template = first.id
                    }
                } catch is CancellationError {
                } catch {
                    self.error = error.localizedDescription
                }
            }
        }
    }

    /// A way to start: its name, what it is for and what it is made of, and
    /// a radio's dot — lit in the accent when it is the one chosen.
    private func choice(_ name: String, detail: String? = nil, facts: String? = nil, chosen: Bool, loading: Bool = false) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: name)
                    .foregroundStyle(Color.kouFg)
                if let detail, !detail.isEmpty {
                    Text(verbatim: detail)
                        .font(.footnote)
                        .foregroundStyle(Color.kouFg3)
                }
                if let facts {
                    Text(verbatim: facts)
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg4)
                }
            }
            Spacer(minLength: 8)
            if loading {
                ProgressView()
                    .controlSize(.small)
                    .tint(Color.kouFg3)
            } else {
                RadioMark(chosen: chosen)
            }
        }
        .contentShape(.rect)
        .accessibilityAddTraits(chosen ? .isSelected : [])
    }

    /// What a template is made of, in the cockpit's mono.
    private static func facts(of template: CanvasTemplate) -> String? {
        var parts: [String] = []
        if let nodes = template.nodes { parts.append("\(nodes) node\(nodes == 1 ? "" : "s")") }
        if let edges = template.edges, edges > 0 { parts.append("\(edges) wire\(edges == 1 ? "" : "s")") }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private var chosen: CanvasTemplate? { model.templates.first { $0.id == template } }

    /// The title of a canvas given none: its template's, or plainly Canvas.
    private var defaultName: String { template == "empty" ? "Canvas" : (chosen?.name ?? "Canvas") }

    private func create() {
        var name = title.trimmingCharacters(in: .whitespacesAndNewlines)
        if name.isEmpty { name = defaultName }
        working = true
        Task {
            defer { working = false }
            do {
                let canvas = try await model.create(title: name, template: template == "empty" ? nil : template)
                dismiss()
                if let canvas { made(canvas) }
            } catch is CancellationError {
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}

/// CanvasRow is a canvas as the list shows it: its mark — the light going
/// round it while an agent of it works —, its title, how long since it
/// changed, and what it is made of.
struct CanvasRow: View {
    let canvas: CanvasSummary

    private var busy: Int { canvas.busy ?? 0 }

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            mark
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(canvas.title.isEmpty ? "Untitled canvas" : canvas.title)
                        .font(.body.weight(.semibold))
                        .foregroundStyle(Color.kouFg)
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if let updated = canvas.updatedAt {
                        TimelineView(.everyMinute) { _ in
                            Text(verbatim: updated.kouAge)
                                .font(.kouMeta)
                                .foregroundStyle(Color.kouFg4)
                        }
                    }
                }
                facts
            }
        }
        .contentShape(.rect)
    }

    /// The canvas's mark: its graph on a tile, the accent's and with the
    /// light going round while an agent works, quiet while none does.
    private var mark: some View {
        let shape = RoundedRectangle(cornerRadius: .corner(10), style: .continuous)
        return Image(systemName: "point.3.filled.connected.trianglepath.dotted")
            .font(.system(size: 16, weight: .medium))
            .foregroundStyle(busy > 0 ? Color.kouAccent : canvas.live == true ? Color.kouFg2 : Color.kouFg4)
            .frame(width: 40, height: 40)
            .background(busy > 0 ? Color.kouAccentSoft : Color.kouPanel, in: shape)
            .overlay(shape.strokeBorder(busy > 0 ? Color.kouAccent.opacity(0.35) : Color.kouLine, lineWidth: 1))
            .runLight(busy > 0, in: shape)
            .accessibilityHidden(true)
    }

    /// What the canvas is made of and what it does now, in the cockpit's
    /// mono: its agents and terminals, the ones working, the ones waiting.
    private var facts: some View {
        HStack(spacing: 8) {
            if busy > 0 {
                HStack(spacing: 2) {
                    LiveDot(size: 6)
                    Text(verbatim: "\(busy) busy")
                        .foregroundStyle(Color.kouAccent)
                }
            } else if canvas.live != true {
                Tag("paused")
            }
            if let waiting = canvas.waiting, waiting > 0 {
                Text(verbatim: "\(waiting) waiting")
                    .foregroundStyle(Color.kouWarn)
            }
            count(canvas.agents, "sparkle")
            count(canvas.terminals, "terminal")
        }
        .font(.kouMeta)
        .foregroundStyle(Color.kouFg3)
        .lineLimit(1)
    }

    @ViewBuilder
    private func count(_ value: Int?, _ symbol: String) -> some View {
        if let value, value > 0 {
            HStack(spacing: 3) {
                Image(systemName: symbol)
                    .imageScale(.small)
                Text(verbatim: "\(value)")
            }
            .accessibilityElement(children: .combine)
        }
    }
}

struct NodeKindIcon: View {
    let kind: String

    var body: some View {
        Image(systemName: Self.symbol(kind))
            .font(.title3)
            .foregroundStyle(kind == CanvasNode.agent ? Color.kouAccent : Color.kouFg3)
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

/// StateBadge is a node's state as the cockpit writes it: a square — lit,
/// hollow or blinking, by the state — and the word, in mono capitals.
struct StateBadge: View {
    let state: String
    /// The word beside its square; the square alone where room is short.
    var word = true
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 6) {
            square
            if word {
                Text(verbatim: state)
                    .textCase(.uppercase)
                    .font(.kouTag)
                    .tracking(1)
                    .foregroundStyle(Self.ink(state))
                    .lineLimit(1)
            }
        }
        .fixedSize()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(state)
    }

    @ViewBuilder
    private var square: some View {
        let tone = Self.color(state)
        let hollow = Self.hollow(state)
        let mark = Rectangle()
            .fill(hollow ? Color.clear : state == "running" ? tone.opacity(0.6) : tone)
            .overlay {
                if hollow {
                    Rectangle().strokeBorder(tone, lineWidth: 1.5)
                }
            }
            .frame(width: 7, height: 7)
        if let beat = Self.beat(state), !reduceMotion {
            mark.phaseAnimator([1.0, 0.15]) { view, opacity in
                view.opacity(opacity)
            } animation: { _ in
                .easeInOut(duration: beat)
            }
        } else {
            mark
        }
    }

    /// The state's colour: the accent at work, the warning's while it waits
    /// for someone, the error's, the done one's when idle, the faint inks
    /// for what does not run.
    static func color(_ state: String) -> Color {
        switch state {
        case "busy", "starting", "running": .kouAccent
        case "waiting", "paused", "proposed": .kouWarn
        case "error": .kouErr
        case "idle": .kouOK
        case "exited": .kouFg3
        default: .kouFg4
        }
    }

    /// The word's colour: a node at rest says so quietly, its square lit.
    static func ink(_ state: String) -> Color {
        switch state {
        case "idle", "running", "exited": .kouFg3
        default: color(state)
        }
    }

    /// The states of a program that does not run: an empty square.
    private static func hollow(_ state: String) -> Bool {
        state == "paused" || state == "exited" || state == "stopped"
    }

    /// How long a blink of the square takes, for the states that blink.
    private static func beat(_ state: String) -> Double? {
        switch state {
        case "busy": 0.5
        case "starting": 0.6
        case "waiting": 0.8
        default: nil
        }
    }
}
