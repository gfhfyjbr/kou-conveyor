import Foundation
import Observation

/// SessionsModel is a workspace's list of sessions, as the cockpit's
/// sidebar shows it.
@MainActor
@Observable
final class SessionsModel {
    let client: RelayClient
    let workspace: String
    private(set) var sessions: [SessionSummary] = []
    private(set) var loaded = false
    private(set) var error: String?

    init(client: RelayClient, workspace: String) {
        self.client = client
        self.workspace = workspace
    }

    /// The sessions the cockpit's list shows: the agents of canvas nodes
    /// are on their canvases.
    var listed: [SessionSummary] { sessions.filter { $0.canvas == nil } }

    var pinned: [SessionSummary] {
        listed.filter { $0.pinned == true }.sorted { ($0.pinOrder ?? 0, $1.lastActive) < ($1.pinOrder ?? 0, $0.lastActive) }
    }

    var recent: [SessionSummary] {
        listed.filter { $0.pinned != true }.sorted { $0.lastActive > $1.lastActive }
    }

    var running: Int { listed.count(where: \.running) }

    func load() async -> (any Error)? {
        do {
            sessions = try await client.get(API.workspace(workspace, "sessions"))
            error = nil
            loaded = true
            return nil
        } catch is CancellationError {
            return nil
        } catch {
            self.error = error.localizedDescription
            loaded = true
            return error
        }
    }

    /// poll keeps the list current while it is on screen: runs start and
    /// end in the cockpit too.
    func poll(noticed: (any Error) -> Void) async {
        while !Task.isCancelled {
            if let error = await load() { noticed(error) }
            try? await Task.sleep(for: .seconds(4))
        }
    }

    func delete(_ session: SessionSummary) async throws {
        var request = client.request("DELETE", API.workspace(workspace, "sessions", session.id))
        request.setValue(nil, forHTTPHeaderField: "Accept")
        _ = try await client.perform(request)
        sessions.removeAll { $0.id == session.id }
    }
}

/// SessionModel is one session: its transcript, its run, and what the user
/// sends it. A run's events come as the cockpit's page has them, through
/// the relay.
@MainActor
@Observable
final class SessionModel {
    enum Phase: Equatable {
        case idle
        /// The prompt is on its way; the server has not named its run yet.
        case starting
        case running
        case stopping
    }

    let client: RelayClient
    let workspace: String
    let id: String
    /// A session the server has not seen yet: its first prompt makes it.
    private(set) var fresh: Bool

    private(set) var entries: [Entry] = []
    private var index: [String: Int] = [:]
    private(set) var title: String?
    private(set) var usage: Usage?
    private(set) var queue: Queue?
    private(set) var phase: Phase = .idle
    private(set) var runID: String?
    private(set) var activity: String?
    private(set) var started: Date?
    /// Run by another process (the terminal cockpit): followed, not driven.
    private(set) var external = false
    private(set) var interrupted = false
    private(set) var loading = false
    private(set) var loadError: String?
    /// A word for the user: a failure, or how a run ended.
    var notice: Notice?
    /// The stream of the run is down and being reconnected.
    private(set) var reconnecting = false

    struct Notice: Equatable, Identifiable {
        let id = UUID()
        let text: String
        let isError: Bool
    }

    private var following: Task<Void, Never>?

    init(client: RelayClient, workspace: String, id: String, fresh: Bool) {
        self.client = client
        self.workspace = workspace
        self.id = id
        self.fresh = fresh
    }

    var isBusy: Bool { phase != .idle }
    var queued: [QueueItem] { queue?.items ?? [] }
    var displayTitle: String {
        if let title, !title.isEmpty { return title }
        if let first = entries.first(where: { $0.kind == Entry.user })?.text, !first.isEmpty {
            return String(first.prefix(60))
        }
        return fresh ? "New session" : "Session"
    }

    // MARK: Loading

    /// load reads the session from the server, and follows its run if it
    /// has one.
    func load() async {
        guard !fresh else { return }
        loading = entries.isEmpty
        defer { loading = false }
        do {
            let detail: SessionDetail = try await client.get(API.workspace(workspace, "sessions", id))
            apply(detail)
            loadError = nil
        } catch is CancellationError {
        } catch RelayError.http(404, _, _) {
            loadError = "This session is gone: it was deleted in the cockpit."
        } catch {
            loadError = error.localizedDescription
        }
    }

    private func apply(_ detail: SessionDetail) {
        title = detail.title
        usage = detail.usage ?? usage
        queue = detail.queue
        external = detail.external == true
        interrupted = detail.interrupted == true
        entries = detail.entries ?? []
        index = Dictionary(entries.enumerated().map { ($1.id, $0) }, uniquingKeysWith: { _, last in last })
        if let run = detail.run {
            if runID != run.id || following == nil {
                follow(run)
            }
        } else if phase != .starting {
            stopFollowing()
            phase = .idle
            runID = nil
        }
    }

    /// watch keeps a session current that the app does not drive: one run
    /// elsewhere, or any while no run's stream is followed.
    func watch() async {
        await load()
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(external ? 2 : 5))
            if Task.isCancelled { return }
            if following == nil, phase == .idle, !fresh { await load() }
        }
    }

    // MARK: Runs

    private func follow(_ run: RunSummary) {
        stopFollowing()
        runID = run.id
        started = run.startedAt ?? Date()
        phase = run.stopping == true ? .stopping : .running
        activity = run.compact == true ? "Compacting context" : (run.stopping == true ? "Stopping" : "Working")
        let runID = run.id
        following = Task { [weak self] in
            await self?.stream(runID)
        }
    }

    private func stopFollowing() {
        following?.cancel()
        following = nil
        reconnecting = false
    }

    private func stream(_ runID: String) async {
        var lastEventID: String?
        var failures = 0
        while !Task.isCancelled, self.runID == runID {
            do {
                for try await event in client.events(API.path("api", "runs", runID, "events"), lastEventID: lastEventID) {
                    if let id = event.id { lastEventID = id }
                    failures = 0
                    reconnecting = false
                    guard let data = event.data.data(using: .utf8),
                          let decoded = try? JSON.decoder().decode(RunEvent.self, from: data) else { continue }
                    if handle(decoded, runID: runID) { return }
                }
                // Every event was sent, and the run ended before this one
                // came: the session file says how.
                ended(runID)
                return
            } catch is CancellationError {
                return
            } catch RelayError.http(404, _, _) {
                // The cockpit no longer knows the run: it ended long ago,
                // or the cockpit restarted.
                ended(runID)
                return
            } catch {
                if Task.isCancelled { return }
                failures += 1
                reconnecting = true
                try? await Task.sleep(for: .milliseconds(min(1500 * failures, 10000)))
            }
        }
    }

    private func ended(_ runID: String) {
        guard self.runID == runID else { return }
        following = nil
        self.runID = nil
        phase = .idle
        reconnecting = false
        Task { await load() }
    }

    /// handle takes a run's event in; it says whether the run is over.
    private func handle(_ event: RunEvent, runID: String) -> Bool {
        switch event.type {
        case "entry":
            if let entry = event.entry { upsert(entry) }
        case "queue":
            queue = event.queue ?? queue
        case "status":
            if let activity = event.activity, !activity.isEmpty { self.activity = activity }
            if let usage = event.usage { self.usage = usage }
            if event.stopping == true { phase = .stopping }
        case "done":
            finish(event, runID: runID)
            return true
        default:
            break
        }
        return false
    }

    private func finish(_ event: RunEvent, runID: String) {
        following = nil
        reconnecting = false
        fresh = false
        if let queue = event.queue { self.queue = queue }
        if let usage = event.usage { self.usage = usage }
        let kind = event.stopped == true ? "stopped" : (event.error?.isEmpty == false ? "failed" : "done")
        interrupted = event.interrupted ?? (kind != "done")
        if let error = event.error, !error.isEmpty {
            notice = Notice(text: error, isError: true)
        } else if kind == "stopped" {
            notice = Notice(text: "The run was stopped.", isError: false)
        }
        if let next = event.next {
            // The next queued message took the session over.
            follow(next)
        } else {
            self.runID = nil
            phase = .idle
            activity = nil
            if event.queue?.paused == true, event.queue?.items?.isEmpty == false {
                notice = Notice(text: "The queue is paused: the run \(kind == "failed" ? "failed" : "was stopped"). Resume it in the cockpit, or send again.", isError: false)
            }
        }
    }

    private func upsert(_ entry: Entry) {
        if let at = index[entry.id], entries.indices.contains(at) {
            entries[at] = entry
        } else {
            index[entry.id] = entries.count
            entries.append(entry)
        }
    }

    // MARK: Sending

    /// send runs text as a prompt; while a run goes on, it waits in the
    /// queue, or, forced, is put to the agent at once.
    func send(_ text: String, force: Bool = false) async -> Bool {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return false }
        if external {
            notice = Notice(text: "This session is running in another window or terminal.", isError: true)
            return false
        }
        if phase == .idle {
            return await start(text)
        }
        guard phase != .starting else {
            notice = Notice(text: "The run is starting; send it again in a moment.", isError: false)
            return false
        }
        do {
            let answer: EnqueueResponse = try await client.send("POST", API.workspace(workspace, "sessions", id, "queue"),
                                                                 body: EnqueueRequest(text: text, force: force))
            if let queue = answer.queue { self.queue = queue }
            if let run = answer.run, run.id != runID { follow(run) }
            if let note = answer.note, !note.isEmpty { notice = Notice(text: note, isError: false) }
            return true
        } catch {
            notice = Notice(text: error.localizedDescription, isError: true)
            return false
        }
    }

    private func start(_ text: String) async -> Bool {
        let messageID = UUID().uuidString.lowercased()
        let pending = Entry(id: "input:\(messageID)", kind: Entry.user, at: Date(), text: text, state: "pending")
        upsert(pending)
        phase = .starting
        activity = "Starting"
        started = Date()
        interrupted = false
        do {
            let answer: StartRunResponse = try await client.send("POST", API.workspace(workspace, "runs"),
                                                                  body: StartRunRequest(prompt: text, sessionID: id, messageID: messageID, resume: !fresh))
            fresh = false
            follow(RunSummary(id: answer.runID, startedAt: started, stopping: nil, compact: nil))
            return true
        } catch {
            phase = .idle
            activity = nil
            var undelivered = pending
            undelivered.state = "undelivered"
            upsert(undelivered)
            switch error as? RelayError {
            case .http(409, _, _):
                notice = Notice(text: "This session is already running elsewhere. Following that run.", isError: false)
                await load()
            case .http(410, _, _):
                notice = Notice(text: "This session was deleted in the cockpit.", isError: true)
            default:
                notice = Notice(text: error.localizedDescription, isError: true)
            }
            return false
        }
    }

    /// stop asks the cockpit to stop the run.
    func stop() async {
        guard let runID, phase == .running else { return }
        phase = .stopping
        activity = "Stopping"
        do {
            try await client.send("POST", API.path("api", "runs", runID, "cancel"), body: Empty())
        } catch RelayError.http(404, _, _) {
            // It ended already; its stream says so.
        } catch {
            if self.runID == runID { phase = .running }
            notice = Notice(text: "Could not stop the run: \(error.localizedDescription)", isError: true)
        }
    }

    func close() {
        stopFollowing()
    }
}
