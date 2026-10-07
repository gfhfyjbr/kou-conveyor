import SwiftUI

/// SessionRoute opens a session: one of the list, or a new one.
struct SessionRoute: Hashable {
    let workspace: String
    let id: String
    let fresh: Bool
}

/// SessionsScreen is the Sessions tab: the workspace's sessions, and the
/// session opened from them.
struct SessionsScreen: View {
    @Environment(AppModel.self) private var app
    @State private var path: [SessionRoute] = []

    var body: some View {
        NavigationStack(path: $path) {
            CockpitGate { client, workspace in
                SessionList(client: client, workspace: workspace, path: $path)
                    .id(workspace.id)
            }
            .navigationTitle("Sessions")
            .navigationDestination(for: SessionRoute.self) { route in
                if let client = app.client {
                    SessionScreen(client: client, route: route)
                }
            }
        }
    }
}

/// WorkspaceMenu switches between the cockpit's workspaces.
struct WorkspaceMenu: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Menu {
            Picker("Workspace", selection: Binding(
                get: { app.workspace?.id ?? "" },
                set: { id in
                    if let workspace = app.workspaces.first(where: { $0.id == id }) { app.select(workspace: workspace) }
                }
            )) {
                ForEach(app.workspaces) { workspace in
                    Label {
                        Text(workspace.name)
                        Text(workspace.display ?? workspace.path)
                    } icon: {
                        Image(systemName: (workspace.running ?? 0) > 0 ? "folder.fill.badge.gearshape" : "folder")
                    }
                    .tag(workspace.id)
                }
            }
        } label: {
            Label(app.workspace?.name ?? "Workspace", systemImage: "folder")
        }
        .disabled(app.workspaces.count < 2)
    }
}

struct SessionList: View {
    @Environment(AppModel.self) private var app
    @State private var model: SessionsModel
    @Binding var path: [SessionRoute]
    let workspace: Workspace
    @State private var query = ""
    @State private var deleting: SessionSummary?

    init(client: RelayClient, workspace: Workspace, path: Binding<[SessionRoute]>) {
        _model = State(initialValue: SessionsModel(client: client, workspace: workspace.id))
        _path = path
        self.workspace = workspace
    }

    var body: some View {
        List {
            let pinned = filtered(model.pinned)
            if !pinned.isEmpty {
                Section("Pinned") {
                    ForEach(pinned) { row($0) }
                }
            }
            let recent = filtered(model.recent)
            if !recent.isEmpty {
                Section(pinned.isEmpty ? "" : "Recent") {
                    ForEach(recent) { row($0) }
                }
            }
        }
        .overlay {
            if !model.loaded {
                ProgressView()
            } else if let error = model.error, model.sessions.isEmpty {
                ContentUnavailableView("Sessions unavailable", systemImage: "exclamationmark.bubble", description: Text(error))
            } else if model.listed.isEmpty {
                ContentUnavailableView {
                    Label("No sessions yet", systemImage: "bubble.left.and.bubble.right")
                } description: {
                    Text("Start one: what you write runs the agent in \(workspace.name), on the computer the cockpit runs on.")
                } actions: {
                    Button("New session", systemImage: "square.and.pencil", action: newSession)
                        .buttonStyle(.glassProminent)
                }
            } else if !query.isEmpty, filtered(model.listed).isEmpty {
                ContentUnavailableView.search(text: query)
            }
        }
        .searchable(text: $query, prompt: "Find a session")
        .refreshable { _ = await model.load() }
        .task { await model.poll(noticed: app.noticed) }
        .navigationSubtitle(subtitle)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                WorkspaceMenu()
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button("New session", systemImage: "square.and.pencil", action: newSession)
            }
        }
        .confirmationDialog("Delete this session?", isPresented: Binding(
            get: { deleting != nil }, set: { if !$0 { deleting = nil } }
        ), titleVisibility: .visible, presenting: deleting) { session in
            Button("Delete “\(session.title.isEmpty ? "Untitled" : session.title)”", role: .destructive) {
                Task {
                    do { try await model.delete(session) } catch { app.noticed(error) }
                }
            }
        } message: { _ in
            Text("Its transcript is deleted from the computer the cockpit runs on.")
        }
    }

    private var subtitle: String {
        let running = model.running
        let place = workspace.display ?? workspace.path
        return running > 0 ? "\(running) running · \(place)" : place
    }

    private func filtered(_ list: [SessionSummary]) -> [SessionSummary] {
        let query = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !query.isEmpty else { return list }
        return list.filter { $0.title.lowercased().contains(query) || $0.id.hasPrefix(query) }
    }

    private func newSession() {
        path.append(SessionRoute(workspace: workspace.id, id: UUID().uuidString.lowercased(), fresh: true))
    }

    private func row(_ session: SessionSummary) -> some View {
        NavigationLink(value: SessionRoute(workspace: workspace.id, id: session.id, fresh: false)) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Group {
                    if session.running {
                        Image(systemName: "circle.dotted.circle")
                            .symbolEffect(.rotate, options: .repeat(.continuous))
                            .foregroundStyle(Color.kouAccent)
                    } else if session.pinned == true {
                        Image(systemName: "pin.fill")
                            .foregroundStyle(.secondary)
                    } else {
                        Image(systemName: "text.bubble")
                            .foregroundStyle(.tertiary)
                    }
                }
                .font(.subheadline)
                .frame(width: 22)
                VStack(alignment: .leading, spacing: 4) {
                    Text(session.title.isEmpty ? "Untitled" : session.title)
                        .font(.body.weight(.medium))
                        .lineLimit(2)
                    HStack(spacing: 6) {
                        if session.running {
                            Text("Running")
                                .foregroundStyle(Color.kouAccent)
                        }
                        Text(session.lastActive, format: .relative(presentation: .named))
                        if let queued = session.queued, queued > 0 {
                            Text("· \(queued) queued\(session.queuePaused == true ? ", paused" : "")")
                        }
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
            }
            .padding(.vertical, 2)
        }
        .swipeActions(edge: .trailing) {
            if !session.running {
                Button("Delete", systemImage: "trash", role: .destructive) { deleting = session }
            }
        }
    }
}
