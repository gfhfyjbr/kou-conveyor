import Foundation
import Observation

/// CanvasesModel is a workspace's canvases.
@MainActor
@Observable
final class CanvasesModel {
    let client: RelayClient
    let workspace: String
    private(set) var canvases: [CanvasSummary] = []
    private(set) var loaded = false
    private(set) var error: String?

    init(client: RelayClient, workspace: String) {
        self.client = client
        self.workspace = workspace
    }

    func load() async -> (any Error)? {
        do {
            let list: CanvasListResponse = try await client.get(API.workspace(workspace, "canvases"))
            canvases = list.canvases.sorted { ($0.updatedAt ?? .distantPast) > ($1.updatedAt ?? .distantPast) }
            error = nil
            loaded = true
            return nil
        } catch is CancellationError {
            return nil
        } catch RelayError.http(404, _, _) {
            // A cockpit started with -canvas off has none.
            canvases = []
            error = "This cockpit runs without canvases (-canvas off)."
            loaded = true
            return nil
        } catch {
            self.error = error.localizedDescription
            loaded = true
            return error
        }
    }

    func poll(noticed: (any Error) -> Void) async {
        while !Task.isCancelled {
            if let error = await load() { noticed(error) }
            try? await Task.sleep(for: .seconds(6))
        }
    }
}

/// CanvasModel is one canvas, live: its nodes and edges, what each node's
/// program does, and the messages between them, from the canvas's event
/// stream.
@MainActor
@Observable
final class CanvasModel {
    let client: RelayClient
    let workspace: String
    let id: String

    private(set) var doc: CanvasDoc?
    private(set) var status: [String: NodeStatus] = [:]
    private(set) var pending: [String: Int] = [:]
    private(set) var messages: [CanvasMessage] = []
    private(set) var readOnly = false
    private(set) var deleted = false
    private(set) var error: String?
    private(set) var reconnecting = false
    /// The latest of each agent's feed, newest last, as the stream brings it.
    private(set) var feeds: [String: [FeedEntry]] = [:]
    /// Bumped by every event about a node, which its sheet reloads on.
    private(set) var touched: [String: Int] = [:]

    private var refetch: Task<Void, Never>?

    init(client: RelayClient, workspace: String, id: String) {
        self.client = client
        self.workspace = workspace
        self.id = id
    }

    var nodes: [CanvasNode] { (doc?.nodes ?? []).sorted { ($0.z ?? 0) < ($1.z ?? 0) } }
    var edges: [CanvasEdge] { doc?.edges ?? [] }

    func node(_ id: String) -> CanvasNode? { doc?.nodes?.first { $0.id == id } }

    func path(_ rest: String...) -> String {
        var path = API.workspace(workspace, "canvases", id)
        for segment in rest { path += API.path(segment) }
        return path
    }

    // MARK: The stream

    /// follow streams the canvas until the view goes: a snapshot first, then
    /// what changes. A dropped stream resumes where it was.
    func follow() async {
        var lastEventID: String?
        var failures = 0
        while !Task.isCancelled, !deleted {
            do {
                for try await event in client.events(path("events"), lastEventID: lastEventID) {
                    if let id = event.id { lastEventID = id }
                    failures = 0
                    reconnecting = false
                    error = nil
                    take(event.data)
                    if deleted { return }
                }
                // The canvas closed the stream (the cockpit stops): again.
                try await Task.sleep(for: .seconds(1))
            } catch is CancellationError {
                return
            } catch RelayError.http(404, _, _) {
                deleted = true
                return
            } catch {
                if Task.isCancelled { return }
                failures += 1
                reconnecting = doc != nil
                if doc == nil { self.error = error.localizedDescription }
                try? await Task.sleep(for: .milliseconds(min(1000 * failures, 8000)))
            }
        }
    }

    /// The type of an event, read first: a snapshot's fields are not those
    /// of the other events (its status is every node's).
    private struct EventKind: Decodable {
        let type: String
    }

    private func take(_ data: String) {
        let bytes = Data(data.utf8)
        let decoder = JSON.decoder()
        guard let kind = try? decoder.decode(EventKind.self, from: bytes) else { return }
        if kind.type == "snapshot" {
            do {
                apply(try decoder.decode(CanvasSnapshot.self, from: bytes))
            } catch {
                if doc == nil { self.error = error.localizedDescription }
            }
            return
        }
        guard let event = try? decoder.decode(CanvasEvent.self, from: bytes) else { return }
        switch event.type {
        case "ops":
            // What changed in the document: the snapshot says it whole.
            scheduleRefetch()
        case "status":
            if let node = event.node, let status = event.status {
                self.status[node] = status
                touch(node)
            }
        case "runtime":
            if let node = event.node, let runtime = event.runtime, var nodes = doc?.nodes,
               let at = nodes.firstIndex(where: { $0.id == node })
            {
                nodes[at].runtime = runtime
                doc = doc.map { CanvasDoc(id: $0.id, title: $0.title, rev: $0.rev, live: $0.live, nodes: nodes, edges: $0.edges) }
                touch(node)
            }
        case "agent":
            if let node = event.node, let entry = event.entry {
                var feed = feeds[node] ?? []
                if let at = feed.firstIndex(where: { $0.id == entry.id }) {
                    feed[at] = entry
                } else {
                    feed.append(entry)
                }
                feeds[node] = Array(feed.suffix(80))
                touch(node)
            }
        case "message":
            if let message = event.message {
                if let at = messages.firstIndex(where: { $0.id == message.id }) {
                    messages[at] = message
                } else {
                    messages.append(message)
                    if messages.count > 200 { messages.removeFirst(messages.count - 200) }
                }
                if let to = message.to?.node { touch(to) }
            }
        case "output", "log", "terminal":
            if let node = event.node { touch(node) }
        case "deleted":
            deleted = true
        default:
            break
        }
    }

    private func apply(_ snapshot: CanvasSnapshot) {
        doc = snapshot.doc
        status = snapshot.status ?? [:]
        pending = snapshot.pending ?? [:]
        messages = Array((snapshot.messages ?? []).suffix(200))
        readOnly = snapshot.readOnly == true
    }

    private func touch(_ node: String) {
        touched[node, default: 0] += 1
    }

    private func scheduleRefetch() {
        refetch?.cancel()
        refetch = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled, let self else { return }
            await self.reload()
        }
    }

    func reload() async {
        do {
            let snapshot: CanvasSnapshot = try await client.get(path())
            apply(snapshot)
        } catch RelayError.http(404, _, _) {
            deleted = true
        } catch {}
    }

    // MARK: Nodes

    /// send writes to a node's program: a message to an agent, a line to a
    /// terminal; keys are keys to press, by name (C-c, enter, escape…).
    func send(to node: String, text: String?, submit: Bool? = nil, keys: [String]? = nil) async throws {
        try await client.send("POST", path("nodes", node, "send"), body: NodeSendRequest(text: text, submit: submit, keys: keys))
    }

    func read(_ node: String, what: String, lines: Int = 200) async throws -> NodeRead {
        try await client.get(path("nodes", node, "read"), query: [
            URLQueryItem(name: "what", value: what), URLQueryItem(name: "lines", value: String(lines)),
        ])
    }

    func transcript(_ node: String, tail: Int = 60) async throws -> NodeTranscript {
        try await client.get(path("nodes", node, "transcript"), query: [URLQueryItem(name: "tail", value: String(tail))])
    }

    func restart(_ node: String, resume: Bool = true) async throws {
        try await client.send("POST", path("nodes", node, "restart"), body: RestartRequest(resume: resume))
    }

    func stop(_ node: String) async throws {
        try await client.send("POST", path("nodes", node, "stop"), body: Empty())
    }
}
