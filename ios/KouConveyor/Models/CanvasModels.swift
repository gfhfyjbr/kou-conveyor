import Foundation
import Observation

/// CanvasesModel is a workspace's canvases, and what is done to one as a
/// whole: made from a template, renamed, paused, deleted.
@MainActor
@Observable
final class CanvasesModel {
    let client: RelayClient
    let workspace: String
    private(set) var canvases: [CanvasSummary] = []
    private(set) var templates: [CanvasTemplate] = []
    private(set) var loaded = false
    private(set) var error: String?
    /// The cockpit runs without canvases (-canvas off): none can be made.
    private(set) var unavailable = false

    init(client: RelayClient, workspace: String) {
        self.client = client
        self.workspace = workspace
    }

    func load() async -> (any Error)? {
        do {
            let list: CanvasListResponse = try await client.get(API.workspace(workspace, "canvases"))
            let sorted = list.canvases.sorted { ($0.updatedAt ?? .distantPast) > ($1.updatedAt ?? .distantPast) }
            // Set only what changed: the list is asked for every few
            // seconds, and each change draws it again.
            if sorted != canvases { canvases = sorted }
            if error != nil { error = nil }
            if unavailable { unavailable = false }
            if !loaded { loaded = true }
            return nil
        } catch is CancellationError {
            return nil
        } catch RelayError.http(404, _, _) {
            // A cockpit started with -canvas off has none.
            canvases = []
            error = "This cockpit runs without canvases (-canvas off)."
            unavailable = true
            loaded = true
            return nil
        } catch {
            let message = error.localizedDescription
            if self.error != message { self.error = message }
            if !loaded { loaded = true }
            return error
        }
    }

    func poll(noticed: (any Error) -> Void) async {
        while !Task.isCancelled {
            if let error = await load() { noticed(error) }
            await AppFront.poll(.seconds(6))
        }
    }

    /// loadTemplates reads what a new canvas can start from: the cockpit's
    /// own templates and those saved from canvases.
    func loadTemplates() async throws {
        let list: CanvasTemplateList = try await client.get(API.workspace(workspace, "canvas", "templates"))
        templates = list.templates
    }

    /// create makes a canvas, empty or from a template.
    func create(title: String, template: String?) async throws -> CanvasSummary? {
        let answer: CreateCanvasResponse = try await client.send("POST", API.workspace(workspace, "canvases"),
                                                                  body: CreateCanvasRequest(title: title, template: template))
        _ = await load()
        return answer.canvas
    }

    func rename(_ canvas: CanvasSummary, to title: String) async throws {
        _ = try await client.send("PATCH", API.workspace(workspace, "canvases", canvas.id), body: CanvasPatch(title: title))
        _ = await load()
    }

    /// setLive runs a canvas, or pauses it: a paused canvas runs no
    /// sources and keeps its messages for later.
    func setLive(_ canvas: CanvasSummary, _ live: Bool) async throws {
        _ = try await client.send("PATCH", API.workspace(workspace, "canvases", canvas.id), body: CanvasPatch(live: live))
        _ = await load()
    }

    /// delete deletes a canvas, its terminals with it; its agents' sessions
    /// too, unless kept.
    func delete(_ canvas: CanvasSummary, keepSessions: Bool) async throws {
        var request = client.request("DELETE", API.workspace(workspace, "canvases", canvas.id),
                                     query: keepSessions ? [URLQueryItem(name: "keep_sessions", value: "1")] : [])
        request.setValue(nil, forHTTPHeaderField: "Accept")
        _ = try await client.perform(request)
        canvases.removeAll { $0.id == canvas.id }
    }
}

/// A word about the canvas for the user: what failed, or what was done
/// that can be undone for a while.
struct CanvasNotice: Identifiable, Equatable {
    let id = UUID()
    let text: String
    var isError = false
    /// The operations that undo what it tells of.
    var undo: [JSONValue]?
}

/// Something that happened on a canvas besides its messages: what the
/// canvas said, what its agents changed.
struct CanvasActivity: Identifiable, Hashable {
    enum Kind: Hashable {
        case notice, change
    }

    let id = UUID()
    let at: Date
    let kind: Kind
    /// A notice's: info, warning or error.
    let level: String?
    let node: String?
    let text: String
}

/// A line a source logged.
struct CanvasLine: Identifiable, Hashable {
    let id = UUID()
    let at: Date
    let text: String
}

/// What a node put out last.
struct CanvasOutput: Hashable {
    let port: String?
    let title: String?
    let preview: String
    let at: Date
}

/// CanvasModel is one canvas, live: its nodes and edges, what each node's
/// program does, the messages between them and those that wait for the
/// user, from the canvas's event stream; and what the user does to it.
@MainActor
@Observable
final class CanvasModel {
    let client: RelayClient
    let workspace: String
    let id: String

    private(set) var doc: CanvasDoc?
    private(set) var status: [String: NodeStatus] = [:]
    /// The messages that wait: for the user's approval, or for their node.
    private(set) var queue: [CanvasMessage] = []
    /// The latest messages, oldest first; their texts may be cut short.
    private(set) var messages: [CanvasMessage] = []
    /// What happened besides the messages, oldest first.
    private(set) var activity: [CanvasActivity] = []
    /// The lines each source logged, and what each node put out last.
    private(set) var logs: [String: [CanvasLine]] = [:]
    private(set) var outputs: [String: CanvasOutput] = [:]
    /// The paths that fire the webhook sources, by node.
    private(set) var hooks: [String: String] = [:]
    /// What can be put on a canvas, and their settings.
    private(set) var kinds: CanvasKinds?
    private(set) var readOnly = false
    private(set) var deleted = false
    private(set) var error: String?
    private(set) var reconnecting = false
    /// The latest of each agent's feed, newest last, as the stream brings it.
    private(set) var feeds: [String: [FeedEntry]] = [:]
    /// Bumped by every event that changes a node, which its sheet reads
    /// again on.
    private(set) var touched: [String: Int] = [:]
    var notice: CanvasNotice?

    /// How many messages wait for each node, as the server counted them:
    /// for a cockpit that sends no queue.
    private var counted: [String: Int] = [:]
    private var queueKnown = false
    @ObservationIgnored private var lastEventID: String?
    @ObservationIgnored private var stream: Task<Void, Never>?
    @ObservationIgnored private var holders = 0
    @ObservationIgnored private var stopping: Task<Void, Never>?
    @ObservationIgnored private var refetch: Task<Void, Never>?

    static let kept = 200

    init(client: RelayClient, workspace: String, id: String) {
        self.client = client
        self.workspace = workspace
        self.id = id
    }

    // MARK: What there is

    var nodes: [CanvasNode] { doc?.nodes ?? [] }
    var edges: [CanvasEdge] { doc?.edges ?? [] }
    var isLive: Bool { doc?.live ?? false }

    func node(_ id: String) -> CanvasNode? { doc?.nodes?.first { $0.id == id } }

    func title(of node: String) -> String {
        guard let found = self.node(node) else { return "a node" }
        return found.title.isEmpty ? found.kind.capitalized : found.title
    }

    func describe(_ edge: CanvasEdge) -> String {
        "«\(title(of: edge.from.node))» → «\(title(of: edge.to.node))»"
    }

    /// The messages that wait for a node.
    func waiting(for node: String) -> [CanvasMessage] { queue.filter { $0.to?.node == node } }

    func pending(for node: String) -> Int {
        queueKnown ? queue.count(where: { $0.to?.node == node }) : counted[node] ?? 0
    }

    func awaiting(for node: String) -> Int {
        queue.count(where: { $0.to?.node == node && $0.awaitsApproval })
    }

    /// The messages that go on only once the user lets them.
    var awaitingApproval: [CanvasMessage] { queue.filter(\.awaitsApproval) }
    /// The nodes agents made that wait for the user's approval.
    var proposed: [CanvasNode] { nodes.filter(\.isProposed) }
    var needsYou: Int { awaitingApproval.count + proposed.count }

    func edges(of node: String) -> [CanvasEdge] {
        edges.filter { $0.from.node == node || $0.to.node == node }
    }

    func harness(of node: CanvasNode) -> HarnessKind? {
        guard node.kind == CanvasNode.terminal else { return nil }
        let preset = node.preset.flatMap { $0.isEmpty ? nil : $0 } ?? "shell"
        let all = kinds?.harnesses ?? []
        return all.first { $0.id == preset && ($0.plugin ?? "") == (node.plugin ?? "") } ?? all.first { $0.id == preset }
    }

    func source(of node: CanvasNode) -> SourceKind? {
        guard node.kind == CanvasNode.source else { return nil }
        let all = kinds?.sources ?? []
        return all.first { $0.id == node.preset && ($0.plugin ?? "") == (node.plugin ?? "") } ?? all.first { $0.id == node.preset }
    }

    /// The settings a node's kind has.
    func schema(of node: CanvasNode) -> ConfigSchema? {
        switch node.kind {
        case CanvasNode.agent: kinds?.agent?.config
        case CanvasNode.terminal: harness(of: node)?.config
        case CanvasNode.source: source(of: node)?.config
        default: nil
        }
    }

    /// A source the user fires by hand.
    static func isManual(_ node: CanvasNode) -> Bool {
        node.kind == CanvasNode.source && node.preset == "manual" && (node.plugin ?? "").isEmpty
    }

    static func takesInput(_ node: CanvasNode) -> Bool {
        node.kind == CanvasNode.agent || node.kind == CanvasNode.terminal
    }

    static func givesOutput(_ node: CanvasNode) -> Bool { node.kind != CanvasNode.note }

    func outPort(of node: CanvasNode) -> String {
        node.kind == CanvasNode.source ? (source(of: node)?.outputs?.first?.id ?? "out") : "out"
    }

    /// freeTitle is base, numbered when a node has it already.
    func freeTitle(_ base: String) -> String {
        let taken = Set(nodes.map { $0.title.lowercased() })
        if !taken.contains(base.lowercased()) { return base }
        var number = 2
        while taken.contains("\(base) \(number)".lowercased()) { number += 1 }
        return "\(base) \(number)"
    }

    func path(_ rest: String...) -> String {
        var path = API.workspace(workspace, "canvases", id)
        for segment in rest { path += API.path(segment) }
        return path
    }

    // MARK: Following

    /// live keeps the canvas current while a screen of it is shown: the
    /// screens of one canvas share its stream, which ends a little after
    /// the last — the canvas and an agent's chat opened from it hand it
    /// over as one goes and the other comes.
    func live() async {
        holders += 1
        stopping?.cancel()
        stopping = nil
        if stream == nil {
            stream = Task { [weak self] in await self?.follow() }
        }
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(3600))
        }
        holders -= 1
        guard holders == 0 else { return }
        stopping = Task { [weak self] in
            try? await Task.sleep(for: .seconds(3))
            guard let self, !Task.isCancelled, self.holders == 0 else { return }
            self.stream?.cancel()
            self.stream = nil
            self.stopping = nil
        }
    }

    /// follow reads the canvas's events: a snapshot first, then each change
    /// as it happens. Reconnecting, it is given what it missed.
    private func follow() async {
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
                // The canvas ended the stream: it was deleted, or the
                // cockpit restarts. Asking again says which.
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
                // Broken while the app was away, it is tried again at once
                // when the app is in front again.
                await AppFront.pause(.milliseconds(min(1000 * failures, 8000)))
            }
        }
    }

    private struct EventKind: Decodable {
        let type: String
    }

    private func take(_ data: String) {
        guard let bytes = data.data(using: .utf8),
              let kind = try? JSON.decoder().decode(EventKind.self, from: bytes) else { return }
        if kind.type == "snapshot" {
            if let snapshot = try? JSON.decoder().decode(CanvasSnapshot.self, from: bytes) {
                apply(snapshot)
            } else {
                scheduleRefetch()
            }
            return
        }
        guard let event = try? JSON.decoder().decode(CanvasEvent.self, from: bytes) else {
            // A change not understood would leave the board wrong until the
            // next snapshot: the canvas is read again whole.
            if kind.type == "ops" || kind.type == "message" { scheduleRefetch() }
            return
        }
        switch event.type {
        case "ops":
            take(ops: event)
        case "status":
            if let node = event.node, let status = event.status {
                self.status[node] = status
                touch(node)
            }
        case "runtime":
            if let node = event.node, let runtime = event.runtime, var doc,
               let at = doc.nodes?.firstIndex(where: { $0.id == node }) {
                doc.nodes?[at].runtime = runtime
                self.doc = doc
                touch(node)
            }
        case "agent":
            if let node = event.node, let entry = event.entry { feed(entry, of: node) }
        case "terminal":
            if let node = event.node { touch(node) }
        case "output":
            if let node = event.node {
                outputs[node] = CanvasOutput(port: event.port, title: event.title,
                                             preview: event.preview ?? event.text ?? "", at: event.at ?? Date())
                touch(node)
            }
        case "message":
            if let message = event.message { take(message: message) }
        case "log":
            if let node = event.node, let text = event.text {
                var lines = logs[node] ?? []
                lines.append(CanvasLine(at: event.at ?? Date(), text: text))
                if lines.count > 100 { lines.removeFirst(lines.count - 100) }
                logs[node] = lines
                touch(node)
            }
        case "notice":
            if let text = event.text, !text.isEmpty {
                let level = event.level ?? "info"
                record(CanvasActivity(at: event.at ?? Date(), kind: .notice, level: level, node: event.node, text: text))
                if level == "error" || level == "warning" {
                    notice = CanvasNotice(text: text, isError: level == "error")
                }
            }
        case "deleted":
            deleted = true
        default:
            break
        }
    }

    /// take(ops:) applies a batch of changes as the canvas made it: nodes
    /// and edges come whole. A batch missed, or one not understood, has the
    /// canvas read again.
    private func take(ops event: CanvasEvent) {
        guard var doc else {
            scheduleRefetch()
            return
        }
        if let rev = event.rev, let have = doc.rev, rev < have { return }
        let gap = event.rev.map { rev in doc.rev.map { rev > $0 + 1 } ?? false } ?? false
        let before = doc
        var nodes = doc.nodes ?? []
        var edges = doc.edges ?? []
        var unknown = false
        for change in event.changes ?? [] {
            switch change.op {
            case "node.add", "node.update":
                guard let node = change.node else {
                    unknown = true
                    continue
                }
                if let at = nodes.firstIndex(where: { $0.id == node.id }) {
                    nodes[at] = node
                } else {
                    nodes.append(node)
                }
                touch(node.id)
            case "node.remove":
                guard let id = change.id else {
                    unknown = true
                    continue
                }
                nodes.removeAll { $0.id == id }
                edges.removeAll { $0.from.node == id || $0.to.node == id }
                status[id] = nil
                feeds[id] = nil
                queue.removeAll { $0.to?.node == id }
            case "edge.add", "edge.update":
                guard let edge = change.edge else {
                    unknown = true
                    continue
                }
                if let at = edges.firstIndex(where: { $0.id == edge.id }) {
                    edges[at] = edge
                } else {
                    edges.append(edge)
                }
                touch(edge.from.node)
                touch(edge.to.node)
            case "edge.remove":
                guard let id = change.id else {
                    unknown = true
                    continue
                }
                edges.removeAll { $0.id == id }
            case "canvas.update":
                guard let set = change.canvas else {
                    unknown = true
                    continue
                }
                if let title = set.title { doc.title = title }
                if let live = set.live { doc.live = live }
                if let settings = set.settings { doc.settings = settings }
            default:
                unknown = true
            }
        }
        doc.nodes = nodes
        doc.edges = edges
        if let rev = event.rev { doc.rev = rev }
        self.doc = doc
        if let actor = event.actor, actor.kind != "user" {
            describe(event.changes ?? [], by: actor, before: before)
        }
        if gap || unknown { scheduleRefetch() }
    }

    /// describe writes what an agent, or the canvas itself, changed into
    /// the activity: the user did not do it.
    private func describe(_ changes: [CanvasChange], by actor: CanvasActor, before: CanvasDoc) {
        let who: String
        if actor.kind == "node", let id = actor.id {
            who = "«\(before.nodes?.first { $0.id == id }?.title ?? title(of: id))»"
        } else {
            who = "The canvas"
        }
        let titled = { (id: String) -> String in
            "«\(before.nodes?.first { $0.id == id }?.title ?? self.title(of: id))»"
        }
        for change in changes {
            var text: String?
            switch change.op {
            case "node.add":
                if let node = change.node {
                    text = node.isProposed ? "\(who) proposes «\(node.title)»: it waits for your approval" : "\(who) added «\(node.title)»"
                }
            case "node.remove":
                if let id = change.id { text = "\(who) removed \(titled(id))" }
            case "edge.add":
                if let edge = change.edge { text = "\(who) wired \(titled(edge.from.node)) to \(titled(edge.to.node))" }
            case "edge.remove":
                if let id = change.id, let edge = before.edges?.first(where: { $0.id == id }) {
                    text = "\(who) unwired \(titled(edge.from.node)) from \(titled(edge.to.node))"
                }
            case "canvas.update":
                text = "\(who) changed the canvas's settings"
            default:
                break
            }
            if let text {
                record(CanvasActivity(at: Date(), kind: .change, level: nil, node: change.node?.id ?? change.id, text: text))
            }
        }
    }

    private func take(message: CanvasMessage) {
        if let at = messages.firstIndex(where: { $0.id == message.id }) {
            messages[at] = message
        } else {
            messages.append(message)
            if messages.count > Self.kept { messages.removeFirst(messages.count - Self.kept) }
        }
        if message.isWaiting {
            if let at = queue.firstIndex(where: { $0.id == message.id }) {
                queue[at] = message
            } else {
                queue.append(message)
            }
        } else {
            queue.removeAll { $0.id == message.id }
        }
        // A cockpit that sends no queue counts the waiting messages only
        // in its snapshots.
        if !queueKnown { scheduleRefetch() }
        if let node = message.to?.node { touch(node) }
    }

    private func apply(_ snapshot: CanvasSnapshot) {
        doc = snapshot.doc
        readOnly = snapshot.readOnly == true
        status = snapshot.status ?? [:]
        counted = snapshot.pending ?? [:]
        if let waiting = snapshot.queue {
            queue = waiting.filter(\.isWaiting)
            queueKnown = true
        } else {
            queue = (snapshot.messages ?? []).filter(\.isWaiting)
            queueKnown = false
        }
        messages = snapshot.messages ?? []
        hooks = snapshot.hooks ?? [:]
        error = nil
        for node in snapshot.doc.nodes ?? [] { touch(node.id) }
    }

    private func feed(_ entry: FeedEntry, of node: String) {
        var list = feeds[node] ?? []
        if let at = list.firstIndex(where: { $0.id == entry.id }) {
            list[at] = entry
        } else {
            list.append(entry)
        }
        if list.count > 30 { list.removeFirst(list.count - 30) }
        feeds[node] = list
    }

    private func touch(_ node: String) {
        touched[node, default: 0] += 1
    }

    private func record(_ item: CanvasActivity) {
        activity.append(item)
        if activity.count > Self.kept { activity.removeFirst(activity.count - Self.kept) }
    }

    private func scheduleRefetch() {
        guard refetch == nil else { return }
        refetch = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard let self, !Task.isCancelled else { return }
            self.refetch = nil
            await self.reload()
        }
    }

    /// reload reads the canvas again whole.
    func reload() async {
        do {
            let snapshot: CanvasSnapshot = try await client.get(path())
            // The stream may have brought newer changes meanwhile.
            if let have = doc?.rev, let got = snapshot.doc.rev, got < have { return }
            apply(snapshot)
        } catch is CancellationError {
        } catch RelayError.http(404, _, _) {
            deleted = true
        } catch {
            if doc == nil { self.error = error.localizedDescription }
        }
    }

    // MARK: Doing

    /// run applies operations to the canvas as one batch; the result has
    /// the ids of the nodes and edges it made, by the index of their
    /// operation.
    @discardableResult
    func run(_ ops: [JSONValue]) async throws -> CanvasBatchResult {
        try await client.send("POST", path("ops"), body: CanvasBatch(ops: ops))
    }

    /// attempt does what the user asked; a failure is told in the notice.
    @discardableResult
    func attempt(_ failure: String, _ work: () async throws -> Void) async -> Bool {
        do {
            try await work()
            return true
        } catch is CancellationError {
            return false
        } catch {
            notice = CanvasNotice(text: "\(failure): \(error.localizedDescription)", isError: true)
            return false
        }
    }

    /// undo does what a notice offers to undo.
    func undo(_ notice: CanvasNotice) async {
        guard let ops = notice.undo else { return }
        if self.notice?.id == notice.id { self.notice = nil }
        await attempt("It was not undone") { _ = try await run(ops) }
    }

    // MARK: Nodes

    /// add puts a node on the canvas, where the canvas finds room for it;
    /// its id, once the board has it.
    func add(_ spec: [String: JSONValue]) async -> String? {
        var made: String?
        await attempt("It was not added") {
            let result = try await run([["op": "node.add", "node": .object(spec)]])
            made = result.ids?["0"]
        }
        guard let made else { return nil }
        await arrived(made)
        return made
    }

    /// arrived waits a moment for a node the canvas made to come down the
    /// stream: an agent's with its session.
    private func arrived(_ id: String) async {
        for _ in 0..<20 {
            if let node = node(id), node.kind != CanvasNode.agent || node.session != nil { return }
            try? await Task.sleep(for: .milliseconds(150))
        }
        await reload()
    }

    @discardableResult
    func update(_ node: String, _ set: [String: JSONValue], failure: String = "It did not change") async -> Bool {
        await attempt(failure) {
            _ = try await run([["op": "node.update", "id": .string(node), "set": .object(set)]])
        }
    }

    @discardableResult
    func rename(_ node: String, to title: String) async -> Bool {
        await update(node, ["title": .string(title)], failure: "It was not renamed")
    }

    /// approve lets a node an agent proposed run.
    @discardableResult
    func approve(node: String) async -> Bool {
        await update(node, ["proposed": false], failure: "It was not approved")
    }

    /// setConfig changes some of a node's settings; .null takes one back
    /// to its default.
    @discardableResult
    func setConfig(_ node: String, _ config: [String: JSONValue]) async -> Bool {
        await update(node, ["config": .object(config)], failure: "The settings were not saved")
    }

    @discardableResult
    func setAccess(_ node: String, _ access: String) async -> Bool {
        await update(node, ["access": .string(access)], failure: "The access did not change")
    }

    /// remove takes a node off the canvas. An agent's session stays,
    /// unless told otherwise; for a few seconds the node can be put back.
    @discardableResult
    func remove(_ node: String, keepSession: Bool = true, discarding: Bool = false) async -> Bool {
        let title = title(of: node)
        let removed = await attempt(discarding ? "It was not discarded" : "It was not removed") {
            let keep: JSONValue = ["session": .bool(keepSession)]
            _ = try await run([["op": "node.remove", "id": .string(node), "keep": keep]])
        }
        if removed {
            let what = discarding ? "discarded" : (keepSession ? "removed" : "removed with its session")
            let restore: JSONValue = ["op": "node.add", "node": ["id": .string(node), "restore": true]]
            notice = CanvasNotice(text: "«\(title)» was \(what).", undo: [restore])
        }
        return removed
    }

    // MARK: Edges

    /// connect wires a node's output to another's input.
    @discardableResult
    func connect(_ from: String, to: String) async -> Bool {
        guard let source = node(from), let target = node(to) else { return false }
        if edges.contains(where: { $0.from.node == from && $0.to.node == to }) {
            notice = CanvasNotice(text: "«\(title(of: from))» is wired to «\(title(of: to))» already.")
            return false
        }
        guard Self.takesInput(target) else { return false }
        let edge: JSONValue = [
            "from": ["node": .string(from), "port": .string(outPort(of: source))],
            "to": ["node": .string(to), "port": "in"],
        ]
        return await attempt("They were not wired") {
            _ = try await run([["op": "edge.add", "edge": edge]])
        }
    }

    @discardableResult
    func update(edge: String, _ set: [String: JSONValue]) async -> Bool {
        await attempt("The wire did not change") {
            _ = try await run([["op": "edge.update", "id": .string(edge), "set": .object(set)]])
        }
    }

    @discardableResult
    func remove(edge: CanvasEdge) async -> Bool {
        let what = describe(edge)
        let removed = await attempt("The wire was not removed") {
            _ = try await run([["op": "edge.remove", "id": .string(edge.id)]])
        }
        if removed {
            var spec: [String: JSONValue] = [
                "id": .string(edge.id),
                "from": ["node": .string(edge.from.node), "port": .string(edge.from.port)],
                "to": ["node": .string(edge.to.node), "port": .string(edge.to.port)],
            ]
            if let mode = edge.mode, !mode.isEmpty { spec["mode"] = .string(mode) }
            if let deliver = edge.deliver, !deliver.isEmpty { spec["deliver"] = .string(deliver) }
            if let template = edge.template, !template.isEmpty { spec["template"] = .string(template) }
            if let header = edge.header { spec["header"] = .bool(header) }
            notice = CanvasNotice(text: "The wire \(what) was removed.", undo: [["op": "edge.add", "edge": .object(spec)]])
        }
        return removed
    }

    // MARK: The canvas

    @discardableResult
    func setLive(_ live: Bool) async -> Bool {
        await updateCanvas(["live": .bool(live)], failure: live ? "It did not go live" : "It was not paused")
    }

    @discardableResult
    func renameCanvas(_ title: String) async -> Bool {
        await updateCanvas(["title": .string(title)], failure: "The canvas was not renamed")
    }

    @discardableResult
    func save(settings: JSONValue) async -> Bool {
        await updateCanvas(["settings": settings], failure: "The settings were not saved")
    }

    /// updateCanvas changes the canvas's title, whether it is live, and its
    /// settings, whole.
    @discardableResult
    func updateCanvas(_ set: [String: JSONValue], failure: String = "The canvas did not change") async -> Bool {
        await attempt(failure) {
            _ = try await run([["op": "canvas.update", "set": .object(set)]])
        }
    }

    /// deleteCanvas deletes the canvas and its terminals; its agents'
    /// sessions too, unless kept.
    @discardableResult
    func deleteCanvas(keepSessions: Bool) async -> Bool {
        await attempt("The canvas was not deleted") {
            var request = client.request("DELETE", path(), query: keepSessions ? [URLQueryItem(name: "keep_sessions", value: "1")] : [])
            request.setValue(nil, forHTTPHeaderField: "Accept")
            _ = try await client.perform(request)
            deleted = true
        }
    }

    /// saveTemplate keeps the canvas as a template new canvases start from.
    @discardableResult
    func saveTemplate(name: String, description: String) async -> Bool {
        await attempt("The template was not saved") {
            _ = try await client.send("POST", path("template"), body: SaveTemplateRequest(name: name, description: description))
        }
    }

    /// askForeman gives the canvas's foreman a task: one is made, with it
    /// as its first prompt, when the canvas has none. The foreman's node.
    func askForeman(_ text: String) async -> String? {
        var node: String?
        await attempt("The foreman was not asked") {
            let answer: ForemanAnswer = try await client.send("POST", path("foreman"), body: ForemanRequest(text: text))
            node = answer.node
        }
        if let node { await arrived(node) }
        return node
    }

    func loadKinds() async {
        guard kinds == nil else { return }
        kinds = try? await client.get(API.workspace(workspace, "canvas", "kinds"), as: CanvasKinds.self)
    }

    /// The sessions of the workspace that could be a node's agent: those
    /// on no canvas, the latest first.
    func freeSessions() async throws -> [SessionSummary] {
        let sessions: [SessionSummary] = try await client.get(API.workspace(workspace, "sessions"))
        return sessions.filter { $0.canvas == nil }.sorted { $0.lastActive > $1.lastActive }
    }

    // MARK: Messages

    /// approve sends a message that waits for the user, as it is or as
    /// edited.
    @discardableResult
    func approve(_ message: CanvasMessage, text: String? = nil) async -> Bool {
        await attempt("It was not sent") {
            let answer: MessageAnswer = try await client.send("POST", path("messages", message.id, "approve"),
                                                              body: ApproveRequest(text: text))
            if let sent = answer.message {
                take(message: sent)
            } else {
                queue.removeAll { $0.id == message.id }
            }
        }
    }

    /// drop drops a message that waits: it never comes to its node.
    @discardableResult
    func drop(_ message: CanvasMessage) async -> Bool {
        await attempt("It was not dropped") {
            var request = client.request("DELETE", path("messages", message.id))
            request.setValue(nil, forHTTPHeaderField: "Accept")
            _ = try await client.perform(request)
            // Shown dropped at once: the stream says so a moment later.
            var dropped = messages.first { $0.id == message.id } ?? message
            dropped.state = CanvasMessage.dropped
            take(message: dropped)
        }
    }

    /// whole is a message with all its text: those the stream brings are
    /// cut short.
    func whole(_ message: CanvasMessage) async throws -> CanvasMessage {
        guard message.truncated == true else { return message }
        return try await client.get(path("messages", message.id))
    }

    // MARK: The nodes' programs

    func send(to node: String, text: String?, submit: Bool? = nil, keys: [String]? = nil, deliver: String? = nil) async throws {
        _ = try await client.send("POST", path("nodes", node, "send"),
                                  body: NodeSendRequest(text: text, submit: submit, keys: keys, deliver: deliver))
    }

    func read(_ node: String, what: String, lines: Int = 200) async throws -> NodeRead {
        try await client.get(path("nodes", node, "read"),
                             query: [URLQueryItem(name: "what", value: what), URLQueryItem(name: "lines", value: String(lines))])
    }

    func transcript(_ node: String, tail: Int = 60) async throws -> NodeTranscript {
        try await client.get(path("nodes", node, "transcript"), query: [URLQueryItem(name: "tail", value: String(tail))])
    }

    /// restart starts a node's program again, resuming its agent's
    /// conversation when the program can.
    @discardableResult
    func restart(_ node: String, resume: Bool = false) async -> Bool {
        await attempt("It did not start") {
            _ = try await client.send("POST", path("nodes", node, "restart"), body: RestartRequest(resume: resume))
        }
    }

    /// stop stops a terminal's program, or a kou agent's run.
    @discardableResult
    func stop(_ node: String) async -> Bool {
        await attempt("It did not stop") {
            _ = try await client.send("POST", path("nodes", node, "stop"), body: Empty())
        }
    }

    /// fire has a manual source put out text.
    @discardableResult
    func fire(_ node: String, text: String) async -> Bool {
        await attempt("It did not fire") {
            _ = try await client.send("POST", path("nodes", node, "fire"), body: FireRequest(text: text))
        }
    }
}
