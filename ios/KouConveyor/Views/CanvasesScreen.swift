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
        List(model.canvases) { canvas in
            NavigationLink(value: CanvasRoute(workspace: workspace.id, id: canvas.id, title: canvas.title)) {
                CanvasRow(canvas: canvas)
            }
            .contextMenu { menu(for: canvas) }
            .swipeActions(edge: .trailing) {
                Button("Delete", systemImage: "trash") { deleting = canvas }
                    .tint(.red)
            }
            .swipeActions(edge: .leading) {
                Button(canvas.live == true ? "Pause" : "Go live", systemImage: canvas.live == true ? "pause.fill" : "play.fill") {
                    setLive(canvas, canvas.live != true)
                }
                .tint(canvas.live == true ? .orange : .green)
            }
        }
        .groupedBackground()
        .overlay {
            if !model.loaded {
                LoadingSpinner()
            } else if let error = model.error, model.canvases.isEmpty {
                ContentUnavailableView("Canvases unavailable", systemImage: "point.3.connected.trianglepath.dotted", description: Text(error))
            } else if model.canvases.isEmpty {
                ContentUnavailableView {
                    Label("No canvases", systemImage: "point.3.connected.trianglepath.dotted")
                } description: {
                    Text("A canvas wires terminals, agents and sources together: what one puts out, the next is given.")
                } actions: {
                    Button("New canvas") { creating = true }
                        .buttonStyle(.glassProminent)
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
            ToolbarItem(placement: .topBarTrailing) {
                Button("New canvas", systemImage: "plus") { creating = true }
                    .disabled(model.unavailable)
            }
        }
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
                Section {
                    TextField("Title", text: $title, prompt: Text(defaultName))
                        .focused($focused)
                        .submitLabel(.done)
                } footer: {
                    Text("Left empty, it is called «\(defaultName)».")
                }
                Section {
                    if model.templates.isEmpty {
                        HStack {
                            Text("Empty")
                            Spacer()
                            if loading {
                                ProgressView()
                            } else {
                                Image(systemName: "checkmark")
                                    .font(.body.weight(.semibold))
                                    .foregroundStyle(Color.kouAccent)
                            }
                        }
                    }
                    ForEach(model.templates) { item in
                        Button {
                            template = item.id
                        } label: {
                            HStack(spacing: 12) {
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(item.name)
                                        .foregroundStyle(Color.primary)
                                    if let description = item.description, !description.isEmpty {
                                        Text(description)
                                            .font(.caption)
                                            .foregroundStyle(Color.secondary)
                                    }
                                }
                                Spacer(minLength: 8)
                                if template == item.id {
                                    Image(systemName: "checkmark")
                                        .font(.body.weight(.semibold))
                                        .foregroundStyle(Color.kouAccent)
                                }
                            }
                        }
                    }
                } header: {
                    Text("Start from")
                }
                if let error {
                    Section {
                        Text(error)
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("New canvas")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Create") { create() }
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
                            .background(Color.kouAccent.opacity(0.18), in: .cornerCapsule)
                            .foregroundStyle(Color.kouAccent)
                    } else {
                        Text("PAUSED")
                            .font(.system(size: 9, weight: .bold, design: .monospaced))
                            .padding(.horizontal, 5)
                            .padding(.vertical, 2)
                            .background(Color.secondary.opacity(0.14), in: .cornerCapsule)
                            .foregroundStyle(Color.secondary)
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
            .background(Self.color(state).opacity(0.15), in: .cornerCapsule)
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
