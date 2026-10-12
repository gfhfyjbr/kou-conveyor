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

    /// rename titles a session; an empty title gives it back the one of its
    /// first prompt.
    func rename(_ session: SessionSummary, to title: String) async throws {
        let _: SessionChanged = try await client.send("PATCH", API.workspace(workspace, "sessions", session.id),
                                                      body: SessionChange(title: title))
        _ = await load()
    }

    func setPinned(_ session: SessionSummary, _ pinned: Bool) async throws {
        let _: SessionChanged = try await client.send("PATCH", API.workspace(workspace, "sessions", session.id),
                                                      body: SessionChange(pinned: pinned))
        _ = await load()
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

    private(set) var entries: [Entry] = [] {
        // What is shown is no longer the answer tagged.
        didSet { shownTag = nil }
    }
    private var index: [String: Int] = [:]
    /// The ETag of the session as read last, while the entries are as it
    /// gave them: the server sends it again only once it changed.
    @ObservationIgnored private var shownTag: String?
    /// How many entries are read: the transcript's last. A session of
    /// thousands, read whole, is megabytes, and laid out from its end it
    /// can show nothing for a while; the earlier come on demand.
    static let page = 200
    @ObservationIgnored private var limit = SessionModel.page
    /// How many entries come before those read, and the prompts among them.
    private(set) var earlier = 0
    private(set) var earlierPrompts: [Entry] = []
    private(set) var loadingEarlier = false
    private(set) var title: String?
    private(set) var pinned = false
    /// The model chosen in this app for the session's next prompts; nil:
    /// the one it ran with last, else the workspace's default.
    private(set) var chosenModel: String?
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
        chosenModel = UserDefaults.standard.string(forKey: Self.modelKey(workspace, id))
    }

    var isBusy: Bool { phase != .idle }
    var queued: [QueueItem] { queue?.items ?? [] }

    // MARK: Model

    private static func modelKey(_ workspace: String, _ id: String) -> String { "model:\(workspace):\(id)" }

    /// The model the session ran its last prompt with.
    var lastModel: String? {
        let ran: (Entry) -> Bool = { $0.kind == Entry.user && $0.state != "undelivered" && $0.model?.isEmpty == false }
        return (entries.last(where: ran) ?? earlierPrompts.last(where: ran))?.model
    }

    /// The model the next prompt runs with, unless the workspace's default.
    var nextModel: String? { chosenModel ?? lastModel }

    /// choose picks the model of the session's next prompts; the running
    /// agent keeps its own. nil goes back to the session's own.
    func choose(model: String?) {
        chosenModel = model
        UserDefaults.standard.set(model, forKey: Self.modelKey(workspace, id))
    }
    var displayTitle: String {
        if let title, !title.isEmpty { return title }
        if let first = (earlierPrompts.first ?? entries.first(where: { $0.kind == Entry.user }))?.text, !first.isEmpty {
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
            // A session read again as it was is not sent again: a big one
            // is megabytes, read every few seconds while idle. What a run
            // added is not read away.
            let last = URLQueryItem(name: "last", value: String(max(limit, entries.count)))
            let (detail, tag) = try await client.get(API.workspace(workspace, "sessions", id), query: [last],
                                                     unless: entries.isEmpty ? nil : shownTag, as: SessionDetail.self)
            if let detail {
                apply(detail)
                shownTag = tag
            }
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
        pinned = detail.pinned == true
        usage = detail.usage ?? usage
        queue = detail.queue
        external = detail.external == true
        interrupted = detail.interrupted == true
        // The same transcript read again is not shown again: thousands of
        // entries, laid out anew every few seconds while idle.
        let read = detail.entries ?? []
        if read != entries {
            entries = read
            index = Dictionary(entries.enumerated().map { ($1.id, $0) }, uniquingKeysWith: { _, last in last })
        }
        earlier = detail.earlier ?? 0
        earlierPrompts = detail.earlierPrompts ?? []
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

    /// showEarlier reads a page more of the entries before those shown.
    func showEarlier() async {
        guard earlier > 0, !loadingEarlier else { return }
        loadingEarlier = true
        defer { loadingEarlier = false }
        limit = entries.count + Self.page
        await load()
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
        activity = run.activity ?? (run.compact == true ? "Compacting context" : (run.stopping == true ? "Stopping" : "Working"))
        let runID = run.id
        let backlog = run.events ?? 0
        following = Task { [weak self] in
            await self?.stream(runID, backlog: backlog)
        }
    }

    private func stopFollowing() {
        following?.cancel()
        following = nil
        reconnecting = false
    }

    /// stream follows a run's events from the first. Those it published
    /// before the session was read (backlog) are taken in at once, as they
    /// leave the session, once the last of them came: one by one, the
    /// thinking row would go through every tool the agent ran so far, and
    /// each tool call back through its states.
    private func stream(_ runID: String, backlog: Int) async {
        var lastEventID: String?
        var failures = 0
        var pending: [RunEvent] = []
        while !Task.isCancelled, self.runID == runID {
            do {
                let events = client.events(API.path("api", "runs", runID, "events"), lastEventID: lastEventID) { event in
                    (event, event.data.data(using: .utf8).flatMap { try? JSON.decoder().decode(RunEvent.self, from: $0) })
                }
                for try await (event, decoded) in events {
                    if let id = event.id { lastEventID = id }
                    failures = 0
                    reconnecting = false
                    guard let decoded else { continue }
                    if let number = event.id.flatMap(Int.init), number < backlog, decoded.type != "done" {
                        pending.append(decoded)
                        continue
                    }
                    if catchUp(&pending, runID: runID) { return }
                    if handle(decoded, runID: runID) { return }
                }
                if catchUp(&pending, runID: runID) { return }
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

    /// catchUp takes the events held back in at once: no await between them,
    /// the screen shows only where they end. It says whether the run is over.
    private func catchUp(_ pending: inout [RunEvent], runID: String) -> Bool {
        defer { pending = [] }
        for event in pending where handle(event, runID: runID) {
            return true
        }
        return false
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
                // A run that failed or was stopped pauses the queue; after one
                // that finished, it was the user who paused it.
                let why = kind == "done" ? "" : ": the run \(kind == "failed" ? "failed" : "was stopped")"
                notice = Notice(text: "The queue is paused\(why). Resume it from the queue's menu, or send again.", isError: false)
            }
        }
    }

    private func upsert(_ entry: Entry) {
        if let at = index[entry.id], entries.indices.contains(at) {
            entries[at] = entry
        } else if earlier > 0, let when = entry.at, let first = entries.first?.at, when < first {
            // One of those not read: a run's events, followed from its
            // first, go back before the entries shown.
        } else {
            index[entry.id] = entries.count
            entries.append(entry)
        }
    }

    // MARK: Sending

    /// send runs text as a prompt, with the images it refers to by their
    /// labels; while a run goes on, it waits in the queue, or, forced, is
    /// put to the agent at once. effort is the run's; nil for the cockpit's.
    func send(_ text: String, force: Bool = false, images: [PromptImage] = [], effort: String? = nil) async -> Bool {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return false }
        let images = images.filter { text.contains($0.label) }
        if external {
            notice = Notice(text: "This session is running in another window or terminal.", isError: true)
            return false
        }
        if phase == .idle {
            return await start(text, images: images, effort: effort)
        }
        guard phase != .starting else {
            notice = Notice(text: "The run is starting; send it again in a moment.", isError: false)
            return false
        }
        do {
            let request = EnqueueRequest(text: text, force: force, model: nextModel, images: images.isEmpty ? nil : images)
            let answer: EnqueueResponse = try await client.send("POST", API.workspace(workspace, "sessions", id, "queue"),
                                                                 body: request)
            if let queue = answer.queue { self.queue = queue }
            if let run = answer.run, run.id != runID { follow(run) }
            if let note = answer.note, !note.isEmpty { notice = Notice(text: note, isError: false) }
            return true
        } catch {
            notice = Notice(text: error.localizedDescription, isError: true)
            return false
        }
    }

    /// Whether a prompt can be edited now: one the session has, while
    /// nothing runs in it.
    func editable(_ entry: Entry) -> Bool {
        entry.messageID != nil && entry.state == nil && phase == .idle && !external && !fresh
    }

    /// How many entries editing a prompt replaces: it and those after it.
    func replaced(by entry: Entry) -> Int {
        guard let at = index[entry.id] else { return 0 }
        return entries.count - at
    }

    /// edit rewinds the session to before a prompt and runs text there
    /// instead; files the agent changed keep their changes. The prompt's
    /// images go again, those text still names; the model is the one
    /// chosen for the session, else the one the prompt ran with.
    func edit(_ entry: Entry, to text: String, effort: String? = nil) async -> Bool {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let message = entry.messageID else { return false }
        guard editable(entry), let at = index[entry.id] else {
            notice = Notice(text: "Wait for the run to end before editing a prompt.", isError: false)
            return false
        }
        let images = await images(of: entry).filter { text.contains($0.label) }
        // The run may have started meanwhile, or the session reloaded.
        guard editable(entry), index[entry.id] == at else {
            notice = Notice(text: "The session changed; edit the prompt again.", isError: false)
            return false
        }
        let before = (entries: entries, index: index, interrupted: interrupted)
        entries.removeSubrange(at...)
        index = index.filter { $0.value < at }
        let model = chosenModel ?? entry.model ?? nextModel
        let started = await start(text, images: images, effort: effort, model: model, rewind: message)
        if !started, entries.count == at + 1, entries.last?.state == "undelivered" {
            // The session is as it was, unless it was read again meanwhile;
            // the edited text goes back to the editor.
            entries = before.entries
            index = before.index
            interrupted = before.interrupted
        }
        return started
    }

    private func start(_ text: String, images: [PromptImage], effort: String?, model: String? = nil, rewind: String? = nil) async -> Bool {
        let messageID = UUID().uuidString.lowercased()
        let model = model ?? nextModel
        let pending = Entry(id: "input:\(messageID)", kind: Entry.user, at: Date(), text: text, state: "pending", model: model)
        upsert(pending)
        phase = .starting
        activity = "Starting"
        started = Date()
        interrupted = false
        do {
            let request = StartRunRequest(prompt: text, sessionID: id, messageID: messageID, resume: !fresh,
                                          model: model, thinkingLevel: effort, images: images.isEmpty ? nil : images,
                                          rewind: rewind)
            let answer: StartRunResponse = try await client.send("POST", API.workspace(workspace, "runs"), body: request)
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
            case let .http(409, _, code) where code == "prompt_gone":
                notice = Notice(text: "That prompt changed in the cockpit.", isError: true)
                await load()
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

    // MARK: The session

    /// rename titles the session; an empty title gives it back the one of
    /// its first prompt.
    func rename(_ title: String) async {
        do {
            let changed: SessionChanged = try await client.send("PATCH", API.workspace(workspace, "sessions", id),
                                                                body: SessionChange(title: title))
            self.title = changed.title
        } catch {
            notice = Notice(text: error.localizedDescription, isError: true)
        }
    }

    func setPinned(_ pinned: Bool) async {
        do {
            let changed: SessionChanged = try await client.send("PATCH", API.workspace(workspace, "sessions", id),
                                                                body: SessionChange(pinned: pinned))
            self.pinned = changed.pinned == true
        } catch {
            notice = Notice(text: error.localizedDescription, isError: true)
        }
    }

    /// The images of a prompt, as the session keeps them: a branch's prompt
    /// goes with them again.
    func images(of entry: Entry) async -> [PromptImage] {
        guard let message = entry.messageID, let infos = entry.images, !infos.isEmpty else { return [] }
        var images: [PromptImage] = []
        for (n, info) in infos.enumerated() {
            guard let label = info.label else { continue }
            let path = API.workspace(workspace, "sessions", id, "images", message, String(n))
            guard let data = try? await client.perform(client.request("GET", path)) else { continue }
            images.append(PromptImage(label: label, mediaType: info.mediaType ?? "image/png", data: data.base64EncodedString()))
        }
        return images
    }

    /// The path of a prompt's image, by its place among the prompt's.
    func imagePath(_ entry: Entry, _ n: Int) -> String? {
        guard let message = entry.messageID, entry.state != "pending" else { return nil }
        return API.workspace(workspace, "sessions", id, "images", message, String(n))
    }

    /// branch starts a session from the history before a prompt, which it
    /// gives back to edit and run; with no prompt, it duplicates the session.
    func branch(before entry: Entry?) async -> BranchResponse? {
        do {
            let request = BranchRequest(messageID: entry?.messageID ?? "")
            return try await client.send("POST", API.workspace(workspace, "sessions", id, "branch"), body: request)
        } catch RelayError.http(409, _, _) {
            notice = Notice(text: "Stop the run before branching the session.", isError: true)
        } catch {
            notice = Notice(text: error.localizedDescription, isError: true)
        }
        return nil
    }

    // MARK: The queue

    /// change edits a queued message: its text, its place, its model; force
    /// puts it to the agent now.
    func change(_ item: QueueItem, _ change: QueueChange) async {
        let path = API.workspace(workspace, "sessions", id, "queue", item.id)
        await queueAction { try await self.client.send("PATCH", path, body: change) }
    }

    /// drop takes a message out of the queue.
    func drop(_ item: QueueItem) async {
        let request = client.request("DELETE", API.workspace(workspace, "sessions", id, "queue", item.id))
        await queueAction { try await self.client.perform(request) }
    }

    /// clearQueue drops every message the agent does not have yet.
    func clearQueue() async {
        let request = client.request("DELETE", API.workspace(workspace, "sessions", id, "queue"))
        await queueAction { try await self.client.perform(request) }
    }

    /// pauseQueue keeps the queued messages for later: none runs when the
    /// run ends.
    func pauseQueue() async {
        let path = API.workspace(workspace, "sessions", id, "queue", "pause")
        await queueAction { try await self.client.send("POST", path, body: Empty()) }
    }

    /// resumeQueue lets the queue go on; with no run, its first message runs.
    func resumeQueue() async {
        let path = API.workspace(workspace, "sessions", id, "queue", "resume")
        await queueAction { try await self.client.send("POST", path, body: Empty()) }
    }

    private func queueAction(_ perform: () async throws -> Data) async {
        do {
            let data = try await perform()
            guard let answer = try? JSON.decoder().decode(EnqueueResponse.self, from: data) else {
                await load()
                return
            }
            if let queue = answer.queue { self.queue = queue }
            if let run = answer.run, run.id != runID { follow(run) }
            if let note = answer.note, !note.isEmpty { notice = Notice(text: note, isError: false) }
        } catch let RelayError.http(404, _, code) where code == "queue_gone" {
            notice = Notice(text: "That message is no longer in the queue.", isError: false)
            await load()
        } catch {
            notice = Notice(text: error.localizedDescription, isError: true)
        }
    }
}
