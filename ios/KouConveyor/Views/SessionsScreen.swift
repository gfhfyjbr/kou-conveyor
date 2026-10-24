import SwiftUI
import UIKit

/// SessionRoute opens a session: one of the list, or a new one.
struct SessionRoute: Hashable {
    let workspace: String
    let id: String
    let fresh: Bool
    /// What the composer starts with: the prompt of a session branched.
    var seed: Seed? = nil
    /// The session of a canvas node: the canvas reserved its id, and it may
    /// have no transcript yet.
    var reserved = false
    /// The model its prompts run with until one is chosen: the node's.
    var model: String? = nil
    /// The title shown instead of the session's own: the node's.
    var title: String? = nil
    /// The session's own title as the list had it, shown until the session
    /// is read: unknown, the screen came in untitled, and its title showed
    /// only once it was in place.
    var listedTitle: String? = nil

    struct Seed: Hashable {
        let prompt: String
        var images: [PromptImage] = []
    }
}

/// SessionsRoute is a page of the Sessions tab, over its workspaces.
enum SessionsRoute: Hashable {
    /// The sessions of a workspace.
    case sessions(String)
    case session(SessionRoute)

    var isSession: Bool {
        if case .session = self { true } else { false }
    }
}

/// SessionsScreen is the Sessions tab: the cockpit's workspaces, the
/// sessions of one over them, and the session opened from those. It opens
/// on the sessions of the workspace shown, as Mail opens on its inbox: a
/// swipe to the right, anywhere on them, takes them aside for the
/// workspaces, listed as the sessions are.
struct SessionsScreen: View {
    @Environment(AppModel.self) private var app
    @State private var path: [SessionsRoute]
    @State private var activity = WorkspacesActivity()

    init() {
        // The tab opens on the sessions of the workspace shown, not on the
        // workspaces for a moment: the one chosen last, known before the
        // cockpit answers, or, none chosen yet, the one the cockpit gives
        // (no ID until it does).
        _path = State(initialValue: [.sessions(AppModel.savedWorkspaceID ?? "")])
    }

    var body: some View {
        NavigationStack(path: $path) {
            CockpitGate { client, _ in
                WorkspaceList(client: client, activity: activity, path: $path)
            }
            .navigationTitle("Workspaces")
            .navigationDestination(for: SessionsRoute.self) { route in
                switch route {
                case let .sessions(id):
                    WorkspaceSessions(id: id, path: $path, activity: activity)
                case let .session(route):
                    if let client = app.client {
                        SessionScreen(client: client, route: route) { path.append(.session($0)) }
                    }
                }
            }
        }
        // A session takes the whole screen: its composer is where the tab
        // bar was.
        .hidesTabBar(when: path.contains(where: \.isSession))
        // The workspace whose sessions are open is the one the app shows:
        // its canvases and its files too.
        .onChange(of: path) { selectOpened() }
        .onChange(of: app.workspace?.id, initial: true) {
            follow()
            openPendingIfReady()
        }
        .onChange(of: app.pendingSession) { openPendingIfReady() }
        // Read before the workspaces page is first shown, once the list the
        // user came for has come.
        .task(id: app.workspaces.map(\.id)) {
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled else { return }
            await activity.prefetch(app)
        }
    }

    /// selectOpened makes the workspace whose sessions were opened the one
    /// the app shows.
    private func selectOpened() {
        guard case let .sessions(id)? = path.first, id != app.workspace?.id,
              let workspace = app.workspaces.first(where: { $0.id == id }) else { return }
        app.select(workspace: workspace)
    }

    /// follow keeps the sessions open those of the workspace the app shows:
    /// given by the cockpit as the tab opened, chosen in another tab, or by
    /// a tapped notification, it takes the place of the one open here. The
    /// workspaces page, once the user went back to it, stays.
    private func follow() {
        guard let current = app.workspace?.id,
              case let .sessions(open)? = path.first, open != current else { return }
        // Put in place, not pushed: it is where the tab is, not a move.
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) { path = [.sessions(current)] }
    }

    /// openPendingIfReady shows the session a tapped turn-end notification
    /// asked for, once its workspace is the one on screen: when the tap
    /// launched the app, the workspaces come only now, and its own is chosen.
    private func openPendingIfReady() {
        guard let pending = app.pendingSession else { return }
        guard pending.workspace == app.workspace?.id else {
            if let match = app.workspaces.first(where: { $0.id == pending.workspace }) {
                // Its change opens the session.
                app.select(workspace: match)
            } else if app.workspacesLoaded {
                // The workspace is closed in the cockpit.
                app.pendingSession = nil
            }
            return
        }
        path = [.sessions(pending.workspace),
                .session(SessionRoute(workspace: pending.workspace, id: pending.id, fresh: false, listedTitle: pending.title))]
        app.pendingSession = nil
    }
}

/// WorkspaceSessions is the sessions page of a workspace, titled with its
/// name; the workspaces page is under it.
private struct WorkspaceSessions: View {
    @Environment(AppModel.self) private var app
    let id: String
    @Binding var path: [SessionsRoute]
    let activity: WorkspacesActivity

    var body: some View {
        // Closed in the cockpit, or not given yet, it is the workspace
        // shown, which the stack opens in its place.
        let workspace = app.workspaces.first { $0.id == id } ?? app.workspace
        CockpitGate { client, current in
            let shown = workspace ?? current
            SessionList(client: client, workspace: shown, path: $path, activity: activity)
                .id(shown.id)
        }
        .navigationTitle(workspace?.name ?? "Sessions")
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
            // The workspace's own monogram, as its row has it: which one the
            // tab shows, at a glance, without an orange folder on every bar.
            // Not in a Label: a bar shows a Label's title for an icon that is
            // not a symbol.
            Monogram(name: app.workspace?.name ?? "·", size: 26)
        }
        .accessibilityLabel(Text(verbatim: app.workspace?.name ?? "Workspace"))
        .accessibilityHint("Shows another workspace")
        .disabled(app.workspaces.count < 2)
    }
}

struct SessionList: View {
    @Environment(AppModel.self) private var app
    @State private var model: SessionsModel
    @Binding var path: [SessionsRoute]
    let workspace: Workspace
    /// The workspaces page's, which learns of the workspace from the list.
    let activity: WorkspacesActivity
    @State private var query = ""
    @State private var deleting: SessionSummary?
    @State private var renaming: SessionSummary?
    @State private var title = ""

    init(client: RelayClient, workspace: Workspace, path: Binding<[SessionsRoute]>, activity: WorkspacesActivity) {
        _model = State(initialValue: SessionsModel(client: client, workspace: workspace.id))
        _path = path
        self.workspace = workspace
        self.activity = activity
    }

    var body: some View {
        List {
            let pinned = filtered(model.pinned)
            if !pinned.isEmpty {
                Section {
                    ForEach(pinned) { row($0) }
                } header: {
                    SectionLabel("Pinned", count: pinned.count)
                        .kouHeader()
                }
            }
            let recent = filtered(model.recent)
            if !recent.isEmpty {
                Section {
                    ForEach(recent) { row($0) }
                } header: {
                    SectionLabel(pinned.isEmpty ? "Sessions" : "Recent", count: recent.count)
                        .kouHeader(top: pinned.isEmpty ? 14 : 22)
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color.kouGround)
        // Hard, so that the rows gone under a pinned header do not show
        // again above it, under the title.
        .scrollEdgeEffectStyle(.hard, for: .top)
        .animation(.snappy, value: model.listed.map(\.id))
        .overlay {
            if !model.loaded {
                LoadingSpinner()
            } else if let error = model.error, model.sessions.isEmpty {
                KouEmpty(symbol: "exclamationmark.bubble", title: "Sessions unavailable", message: error, tone: .kouErr)
            } else if model.listed.isEmpty {
                KouEmpty(symbol: "text.bubble", title: "No sessions yet",
                         message: "What you write runs the agent in \(workspace.name), on the computer the cockpit runs on.") {
                    Button(action: newSession) {
                        Label("New session", systemImage: "square.and.pencil")
                            .font(.body.weight(.semibold))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 4)
                    }
                    .buttonStyle(.glassProminent)
                    .buttonBorderShape(.cornerCapsule)
                }
            } else if !query.isEmpty, filtered(model.listed).isEmpty {
                KouEmpty(symbol: "magnifyingglass", title: "Nothing called “\(query)”",
                         message: "No session of \(workspace.name) has it in its title.")
            }
        }
        .searchable(text: $query, prompt: "Find a session")
        .refreshable { _ = await model.load() }
        .task { await model.poll(noticed: app.noticed) }
        .onChange(of: model.sessions) { activity.note(workspace.id, model.sessions) }
        .navigationSubtitle(subtitle)
        // The other workspaces are under the list: back, or a swipe to the
        // right anywhere on it.
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("New session", systemImage: "square.and.pencil", action: newSession)
                    .buttonStyle(.glassProminent)
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
        .alert("Rename the session", isPresented: Binding(
            get: { renaming != nil }, set: { if !$0 { renaming = nil } }
        ), presenting: renaming) { session in
            TextField("Title", text: $title)
            Button("Rename") {
                let title = title
                Task {
                    do { try await model.rename(session, to: title) } catch { app.noticed(error) }
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("Empty, it takes the first prompt for its title again.")
        }
    }

    private func rename(_ session: SessionSummary) {
        title = session.title
        renaming = session
    }

    private func pin(_ session: SessionSummary) {
        Task {
            do { try await model.setPinned(session, session.pinned != true) } catch { app.noticed(error) }
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
        path.append(.session(SessionRoute(workspace: workspace.id, id: UUID().uuidString.lowercased(), fresh: true)))
    }

    private func row(_ session: SessionSummary) -> some View {
        NavigationLink(value: SessionsRoute.session(SessionRoute(workspace: workspace.id, id: session.id, fresh: false, listedTitle: session.title))) {
            SessionRow(session: session)
        }
        .navigationLinkIndicatorVisibility(.hidden)
        .listRowBackground(Color.kouGround)
        .listRowSeparatorTint(Color.kouLine)
        .listRowInsets(EdgeInsets(top: 12, leading: 14, bottom: 12, trailing: 18))
        // All on the trailing edge: a swipe to the right on a row takes the
        // whole list aside, for the workspaces.
        .swipeActions(edge: .trailing) {
            if !session.running {
                Button("Delete", systemImage: "trash", role: .destructive) { deleting = session }
            }
            Button("Rename", systemImage: "pencil") { rename(session) }
                .tint(.gray)
            Button(session.pinned == true ? "Unpin" : "Pin", systemImage: session.pinned == true ? "pin.slash" : "pin") {
                pin(session)
            }
            .tint(Color.kouAccent)
        }
        .contextMenu {
            Button("Rename…", systemImage: "pencil") { rename(session) }
            Button(session.pinned == true ? "Unpin" : "Pin", systemImage: session.pinned == true ? "pin.slash" : "pin") {
                pin(session)
            }
            Button("Copy session ID", systemImage: "doc.on.doc") { UIPasteboard.general.string = session.id }
            if !session.running {
                Divider()
                Button("Delete…", systemImage: "trash", role: .destructive) { deleting = session }
            }
        }
    }
}

/// SessionRow is a session as the list shows it: a mark of what it is —
/// running, pinned, or a quiet dot —, its title, how long since it was
/// written to, and what waits in its queue.
struct SessionRow: View {
    let session: SessionSummary

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            mark
                .frame(width: 18)
                // On the title's first line, whatever its size.
                .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 5 }
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Text(verbatim: session.title.isEmpty ? "Untitled" : session.title)
                        .font(.body.weight(session.running ? .semibold : .regular))
                        .foregroundStyle(Color.kouFg)
                        .lineLimit(2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    TimelineView(.everyMinute) { _ in
                        Text(verbatim: session.lastActive.kouAge)
                            .font(.kouMeta)
                            .foregroundStyle(session.running ? Color.kouFg2 : Color.kouFg4)
                    }
                    .fixedSize()
                }
                if let facts {
                    facts
                }
            }
            // The row's separator starts under its title, not under its mark.
            .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
        }
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder private var mark: some View {
        if session.running {
            LiveDot(size: 7)
        } else if session.pinned == true {
            Diamond()
                .fill(Color.kouFg3)
                .frame(width: 7, height: 7)
        } else {
            Circle()
                .strokeBorder(Color.kouFg4, lineWidth: 1)
                .frame(width: 7, height: 7)
        }
    }

    /// What the session is up to, under its title: running, its queue, a
    /// canvas's node.
    private var facts: AnyView? {
        let queued = session.queued ?? 0
        guard session.running || queued > 0 || session.canvas != nil else { return nil }
        return AnyView(HStack(spacing: 8) {
            if session.running {
                Eyebrow("Running", color: .kouAccent)
            }
            if queued > 0 {
                Eyebrow(session.queuePaused == true ? "\(queued) queued · paused" : "\(queued) queued",
                        color: session.queuePaused == true ? .kouWarn : .kouFg3)
            }
            if session.canvas != nil {
                Tag("canvas", tone: .kouFg3)
            }
        })
    }
}
