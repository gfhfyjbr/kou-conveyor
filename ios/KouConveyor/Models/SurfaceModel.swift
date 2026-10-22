import Foundation
import Observation

/// A UI call of a transcript: the entry it is, and what its output said.
struct SurfaceRef: Equatable, Sendable {
    let entry: String
    let call: SurfaceCall
}

/// A source of a surface: a file of the workspace, and what it is read as.
struct SurfaceSourceSpec: Equatable, Sendable {
    let name: String
    let file: String
    let format: String
}

/// What went wrong reading a surface, as it is said.
struct SurfaceProblem: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// SurfaceVersion is a version of a surface as the plugin keeps it,
/// .harness/ui/<session>/<surface>/v<N>.json: what the call drew, the data
/// it set, and the files as the call read them — its snapshot.
struct SurfaceVersion: Sendable {
    var surface = ""
    var version = 0
    var title = ""
    var glance = ""
    var at: Date?
    var deleted = false
    var sources: [SurfaceSourceSpec] = []
    var components: [String: SurfaceObject] = [:]
    var data = SurfaceObject()
    /// The sources as the call read them, those it found.
    var snapshotSources = SurfaceObject()
    /// What the call found of each: path, format, size, mtime, error.
    var snapshotInfo = SurfaceObject()
    /// The diffs the call took, by the Diff's id.
    var diffs = SurfaceObject()
    /// The surface as text, as the agent read it.
    var text = ""

    init(_ value: SurfaceValue) throws {
        guard let o = value.object else { throw SurfaceProblem(message: "This is not a surface's version.") }
        surface = o.string("surface") ?? ""
        version = o.int("version") ?? 0
        title = o.string("title") ?? ""
        glance = o.string("glance") ?? ""
        if let at = o.string("at") { self.at = try? Date(at, strategy: .iso8601) }
        deleted = o.bool("deleted") == true
        for (name, spec) in o.object("sources")?.pairs ?? [] {
            guard let spec = spec.object, let file = spec.string("file"), !file.isEmpty else { continue }
            sources.append(SurfaceSourceSpec(name: name, file: file, format: spec.string("format") ?? SurfaceSource.format(of: file)))
        }
        for c in o.array("components") ?? [] {
            if let c = c.object, let id = c.string("id") { components[id] = c }
        }
        data = o.object("data") ?? SurfaceObject()
        let snapshot = o.object("snapshot")
        for (name, value) in snapshot?.object("sources")?.pairs ?? [] where !value.isNull {
            snapshotSources[name] = value
        }
        snapshotInfo = snapshot?.object("sourceInfo") ?? SurfaceObject()
        diffs = snapshot?.object("diffs") ?? SurfaceObject()
        text = o.string("text") ?? ""
    }
}

/// What a source of a live surface read last.
struct SurfaceLive: Equatable, Sendable {
    /// The file's version, its size and time: the same, it is not parsed again.
    var stamp: String?
    /// Its value; nil while there is none: no file yet, or one that does not parse.
    var value: SurfaceValue?
    /// /sourceInfo/<name>: path, format, size, mtime and error.
    var info: SurfaceObject
    var modified: Date?

    var error: String? { info.string("error") }
}

/// A file a Code component shows, as it is now.
struct SurfaceFile: Equatable, Sendable {
    var stamp: String?
    var text: String?
    var runs: [Int32]?
    var classes: [String]?
    var error: String?
}

enum SurfaceLoad: Sendable {
    case loading
    case ready(SurfaceVersion)
    case failed(String)
}

/// SurfaceBook keeps the surfaces of a session's transcript, as the web
/// cockpit's genui plugin does: which call of each surface is its newest —
/// drawn live, while the calls before it are snapshots — the versions as
/// read, the files the live ones read as they change, and what the user did
/// to them: what the inputs set, and the buttons that work once.
@MainActor
@Observable
final class SurfaceBook {
    /// The newest call of each surface: surface → entry id.
    private(set) var heads: [String: String] = [:]
    /// The UI calls of the transcript, in its order.
    private(set) var calls: [SurfaceRef] = []
    /// The versions read, by path.
    private(set) var versions: [String: SurfaceLoad] = [:]
    /// The sources of the live versions as read: path → source → state.
    private(set) var live: [String: [String: SurfaceLive]] = [:]
    /// The files Code components show, by path in the workspace.
    private(set) var files: [String: SurfaceFile] = [:]
    /// What the inputs of each version set: path → pointer → value.
    private(set) var edits: [String: SurfaceObject] = [:]
    /// The buttons that work once and were used: "<path>#<component>".
    private(set) var used: Set<String>

    @ObservationIgnored private weak var session: SessionModel?
    @ObservationIgnored private(set) var client: RelayClient?
    @ObservationIgnored private(set) var workspace = ""

    private static let usedKey = "surfaces.used"
    private static let usedKept = 400

    init() {
        used = Set(UserDefaults.standard.stringArray(forKey: Self.usedKey) ?? [])
    }

    /// attach ties the book to the session it draws the surfaces of.
    func attach(_ session: SessionModel) -> SurfaceBook {
        if self.session !== session {
            self.session = session
            client = session.client
            workspace = session.workspace
        }
        return self
    }

    /// calls are the UI calls of a transcript that showed something.
    static func calls(in entries: [Entry]) -> [SurfaceRef] {
        entries.compactMap { entry in
            guard entry.kind == Entry.tool, let tool = entry.tool, tool.state == "done",
                  let call = SurfaceCall(tool: tool) else { return nil }
            return SurfaceRef(entry: entry.id, call: call)
        }
    }

    func sync(_ calls: [SurfaceRef]) {
        var heads: [String: String] = [:]
        for ref in calls { heads[ref.call.surface] = ref.entry }
        if heads != self.heads { self.heads = heads }
        if calls != self.calls { self.calls = calls }
    }

    func isHead(_ entry: String, _ surface: String) -> Bool { heads[surface] == entry }

    /// newest is the newest call of a surface.
    func newest(_ surface: String) -> SurfaceCall? {
        guard let id = heads[surface] else { return nil }
        return calls.last { $0.entry == id }?.call
    }

    // MARK: Reading

    /// load reads a version once: a version's file never changes.
    func load(_ path: String) async {
        switch versions[path] {
        case .ready?, .loading?: return
        default: break
        }
        guard let client else { return }
        versions[path] = .loading
        do {
            let file: FileView = try await client.get(API.workspace(workspace, "file"), query: [URLQueryItem(name: "path", value: path)])
            guard let text = file.text else { throw SurfaceProblem(message: "\(path) is not text.") }
            let version = try await Task.detached(priority: .userInitiated) {
                try SurfaceVersion(SurfaceJSON.parse(text))
            }.value
            versions[path] = .ready(version)
        } catch {
            if Task.isCancelled {
                versions[path] = nil
            } else if case let RelayError.http(status, _, _) = error, status == 404 {
                versions[path] = .failed("There is no \(path) any more.")
            } else {
                versions[path] = .failed(Self.say(error))
            }
        }
    }

    /// watch reads the files of the live surfaces again and again while the
    /// session is on screen.
    func watch() async {
        while !Task.isCancelled {
            await refresh()
            await AppFront.poll(.seconds(1.5))
        }
    }

    private func refresh() async {
        for ref in calls where heads[ref.call.surface] == ref.entry && !ref.call.deleted {
            guard case let .ready(version)? = versions[ref.call.path] else { continue }
            for spec in version.sources {
                if Task.isCancelled { return }
                await read(ref.call.path, spec)
            }
        }
        for file in files.keys.sorted() {
            if Task.isCancelled { return }
            await readFile(file)
        }
    }

    private func read(_ path: String, _ spec: SurfaceSourceSpec) async {
        guard let client else { return }
        let before = live[path]?[spec.name]
        var state = SurfaceLive(info: SurfaceObject([("path", .string(spec.file)), ("format", .string(spec.format))]))
        do {
            let file: FileView = try await client.get(API.workspace(workspace, "file"), query: [URLQueryItem(name: "path", value: spec.file)])
            if let stamp = file.version, stamp == before?.stamp { return }
            state.stamp = file.version
            state.modified = file.modified
            if let size = file.size { state.info["size"] = .number(Double(size)) }
            if let modified = file.modified { state.info["mtime"] = .string(modified.formatted(.iso8601)) }
            if file.tooLarge == true {
                state.info["error"] = .string("bigger than 8 MB")
            } else if let text = file.text {
                let format = spec.format
                let parsed = await Task.detached(priority: .utility) { () -> Result<SurfaceValue, any Error> in
                    Result { try SurfaceSource.parse(format, text) }
                }.value
                switch parsed {
                case let .success(value):
                    state.value = value
                    if format == "text" || format == "markdown", text.count > SurfaceSource.maxText {
                        state.info["cut"] = .bool(true)
                    }
                case let .failure(error):
                    state.info["error"] = .string("not \(format): \(Self.say(error))")
                }
            } else {
                state.info["error"] = .string("not text")
            }
        } catch {
            if Task.isCancelled { return }
            guard case let RelayError.http(status, message, _) = error else {
                // A relay that does not answer keeps what was read.
                return
            }
            state.info["error"] = .string(status == 404 ? "no such file" : message)
        }
        if state != before { live[path, default: [:]][spec.name] = state }
    }

    /// want reads a file a Code component shows, unless it was read: from
    /// then on it is read again with the sources.
    func want(_ file: String) async {
        if files[file] == nil { await readFile(file) }
    }

    private func readFile(_ file: String) async {
        guard let client else { return }
        let before = files[file]
        var got = SurfaceFile()
        do {
            let view: FileView = try await client.get(API.workspace(workspace, "file"), query: [URLQueryItem(name: "path", value: file)])
            if let stamp = view.version, stamp == before?.stamp { return }
            got.stamp = view.version
            if let text = view.text {
                got.text = text
                got.runs = view.runs
                got.classes = view.classes
            } else {
                got.error = view.tooLarge == true ? "\(file) is bigger than 8 MB." : "\(file) is not text."
            }
        } catch {
            if Task.isCancelled { return }
            if case let RelayError.http(status, _, _) = error, status == 404 {
                got.error = "There is no \(file)."
            } else if before != nil {
                return
            } else {
                got.error = Self.say(error)
            }
        }
        if got != before { files[file] = got }
    }

    // MARK: What the user did

    /// set is what an input does: it sets a pointer of the data model,
    /// outside the sources.
    func set(_ path: String, _ pointer: String, _ value: SurfaceValue) {
        guard let first = SurfacePointer.tokens(pointer)?.first, first != "sources", first != "sourceInfo" else { return }
        var kept = edits[path] ?? SurfaceObject()
        kept[pointer] = value
        edits[path] = kept
    }

    func edited(_ path: String) -> Bool { !(edits[path]?.isEmpty ?? true) }

    func reset(_ path: String) { edits[path] = nil }

    func isUsed(_ path: String, _ id: String) -> Bool { used.contains("\(path)#\(id)") }

    func use(_ path: String, _ id: String) {
        let key = "\(path)#\(id)"
        guard !used.contains(key) else { return }
        used.insert(key)
        var kept = UserDefaults.standard.stringArray(forKey: Self.usedKey) ?? []
        kept.append(key)
        if kept.count > Self.usedKept { kept.removeFirst(kept.count - Self.usedKept) }
        UserDefaults.standard.set(kept, forKey: Self.usedKey)
    }

    /// doc is the data model a version is drawn from: its data, what the
    /// inputs set, and its sources — the live ones as the files are now,
    /// as long as they were read, a snapshot's as the call read them.
    func doc(_ call: SurfaceCall, _ version: SurfaceVersion, live isLive: Bool) -> SurfaceValue {
        var doc = version.data
        var sources = SurfaceObject()
        var info = SurfaceObject()
        if isLive {
            for (pointer, value) in edits[call.path]?.pairs ?? [] {
                SurfacePointer.set(&doc, pointer, value)
            }
            let read = live[call.path] ?? [:]
            for spec in version.sources {
                if let state = read[spec.name] {
                    if let value = state.value { sources[spec.name] = value }
                    info[spec.name] = .object(state.info)
                } else {
                    if let value = version.snapshotSources[spec.name] { sources[spec.name] = value }
                    if let found = version.snapshotInfo[spec.name] { info[spec.name] = found }
                }
            }
        } else {
            sources = version.snapshotSources
            info = version.snapshotInfo
        }
        doc["sources"] = .object(sources)
        doc["sourceInfo"] = .object(info)
        return .object(doc)
    }

    // MARK: The agent

    /// send gives the agent a message: a run of its own when the session is
    /// idle, the queue's otherwise.
    func send(_ text: String) async -> Bool {
        guard let session else { return false }
        return await session.send(text)
    }

    /// event sends what a Button's event says, with the values its data
    /// binds as they are now.
    func event(_ call: SurfaceCall, _ doc: SurfaceValue, _ c: SurfaceObject, _ event: SurfaceObject) async -> Bool {
        guard let name = event.string("name"), !name.isEmpty else { return false }
        let data = SurfaceCall.eventData(doc, event["data"])
        let text = SurfaceCall.eventText(surface: call.surface, version: call.version, name: name, data: .object(data))
        let sent = await send(text)
        if sent, c.bool("once") == true, let id = c.string("id") { use(call.path, id) }
        return sent
    }

    static func say(_ error: any Error) -> String {
        if let error = error as? SurfaceJSONError { return error.description }
        return error.localizedDescription
    }
}
