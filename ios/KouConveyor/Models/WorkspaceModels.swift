import Foundation
import Observation

/// WorkspaceSummary is what a workspace's sessions tell of it: how many
/// there are, how many run, and the one written to last.
struct WorkspaceSummary: Equatable, Sendable {
    /// The sessions its list shows: the agents of canvas nodes are on their
    /// canvases.
    let sessions: Int
    let running: Int
    /// The session the user wrote to last.
    let latest: SessionSummary?

    init(_ list: [SessionSummary]) {
        let listed = list.filter { $0.canvas == nil }
        sessions = listed.count
        running = listed.count(where: \.running)
        latest = listed.max { $0.lastActive < $1.lastActive }
    }

    /// When the user last wrote in the workspace, which orders the list.
    var lastActive: Date? { latest?.lastActive }
}

/// WorkspacesActivity is what goes on in each of the cockpit's workspaces:
/// the workspaces page shows it on their rows. It outlives the page, so the
/// rows come back whole and in their order as the page comes back.
@MainActor
@Observable
final class WorkspacesActivity {
    private(set) var summaries: [String: WorkspaceSummary] = [:]
    /// The workspaces whose sessions did not come when last asked for.
    private(set) var unavailable: Set<String> = []
    /// When all of them were last asked for.
    @ObservationIgnored private var loadedAt: Date?

    /// How many workspaces are asked at once: the relay carries them all,
    /// with the screen's own requests.
    private static let width = 4

    /// note takes the sessions of a workspace read elsewhere: those of its
    /// own page, every few seconds while it is shown.
    func note(_ workspace: String, _ sessions: [SessionSummary]) {
        apply(workspace, WorkspaceSummary(sessions))
    }

    /// load asks for the sessions of each workspace, a few at a time.
    func load(_ client: RelayClient, _ workspaces: [Workspace]) async {
        let ids = workspaces.map(\.id)
        await withTaskGroup(of: (String, WorkspaceSummary?).self) { group in
            var waiting = ids.makeIterator()
            for _ in 0..<Self.width {
                guard let id = waiting.next() else { break }
                group.addTask { await Self.fetch(client, id) }
            }
            while let result = await group.next() {
                let (id, summary) = result
                // Left as the page went: what did not come is not lost.
                if !Task.isCancelled { apply(id, summary) }
                if let next = waiting.next() {
                    group.addTask { await Self.fetch(client, next) }
                }
            }
        }
        guard !Task.isCancelled else { return }
        loadedAt = Date()
        // Workspaces closed in the cockpit since; set only if there are any,
        // as each change draws the rows again.
        for id in summaries.keys.filter({ !ids.contains($0) }) {
            summaries[id] = nil
        }
        let gone = unavailable.subtracting(ids)
        if !gone.isEmpty { unavailable.subtract(gone) }
    }

    /// prefetch asks once, unless it did a moment ago: the workspaces are
    /// read before their page is first shown, so its rows do not fill in,
    /// and move to their order, under the user's eyes.
    func prefetch(_ app: AppModel) async {
        if let loadedAt, Date().timeIntervalSince(loadedAt) < 30 { return }
        guard let client = app.client, !app.workspaces.isEmpty else { return }
        await load(client, app.workspaces)
    }

    /// poll keeps the summaries current while the workspaces are shown: runs
    /// start and end, and prompts are sent, in the cockpit too.
    func poll(_ app: AppModel) async {
        // Read a moment ago, as the page came: not again at once. A swipe
        // that only peeks at the page does not ask for them all each time.
        if let loadedAt, Date().timeIntervalSince(loadedAt) < 5 {
            await AppFront.poll(.seconds(5))
        }
        while !Task.isCancelled {
            if let client = app.client, !app.workspaces.isEmpty {
                await load(client, app.workspaces)
            }
            await AppFront.poll(.seconds(8))
        }
    }

    private func apply(_ id: String, _ summary: WorkspaceSummary?) {
        guard let summary else {
            if !unavailable.contains(id) { unavailable.insert(id) }
            return
        }
        // Set only what changed: each change draws the rows again.
        if summaries[id] != summary { summaries[id] = summary }
        if unavailable.contains(id) { unavailable.remove(id) }
    }

    nonisolated private static func fetch(_ client: RelayClient, _ id: String) async -> (String, WorkspaceSummary?) {
        do {
            let list: [SessionSummary] = try await client.get(API.workspace(id, "sessions"))
            return (id, WorkspaceSummary(list))
        } catch {
            return (id, nil)
        }
    }
}
