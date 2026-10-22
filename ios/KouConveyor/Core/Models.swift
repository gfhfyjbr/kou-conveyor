import CoreGraphics
import Foundation

// The cockpit's API, as kou-conveyor-web serves it (cmd/kou-conveyor-web,
// cmd/internal/cockpit, cmd/internal/canvas). Keys are spelled out: a key
// strategy would rewrite the keys of maps too, which are node IDs.

// MARK: Workspaces

struct Workspace: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let name: String
    let path: String
    let display: String?
    let startup: Bool?
    let missing: Bool?
    let running: Int?
    let connection: Connection?
    /// Whether the cockpit takes dictation in the workspace: Soniox's key is
    /// set (voice.go).
    let voice: Bool?

    struct Connection: Decodable, Sendable, Hashable {
        let source: String?
        let provider: String?
        let model: String?
    }
}

// MARK: Sessions

struct SessionSummary: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let title: String
    let updatedAt: Date?
    let promptedAt: Date?
    let size: Int64?
    let pinned: Bool?
    let pinOrder: Int?
    let canvas: String?
    let node: String?
    let runID: String?
    let queued: Int?
    let queuePaused: Bool?

    var running: Bool { runID != nil }
    /// When the user last wrote to it, which orders the list.
    var lastActive: Date { promptedAt ?? updatedAt ?? .distantPast }

    enum CodingKeys: String, CodingKey {
        case id, title, size, pinned, canvas, node, queued
        case updatedAt = "updated_at", promptedAt = "prompted_at", pinOrder = "pin_order"
        case runID = "run_id", queuePaused = "queue_paused"
    }
}

struct SessionDetail: Decodable, Sendable {
    let id: String
    let workspace: String?
    let title: String?
    let entries: [Entry]?
    let usage: Usage?
    let run: RunSummary?
    let size: Int64?
    let external: Bool?
    let pinned: Bool?
    let queue: Queue?
    let renamed: Bool?
    let interrupted: Bool?
    let canvas: String?
    let node: String?
    /// Read from its end (?last=), how many entries come before those
    /// given, and the prompts among them.
    let earlier: Int?
    let earlierPrompts: [Entry]?

    enum CodingKeys: String, CodingKey {
        case id, workspace, title, entries, usage, run, size, external, pinned, queue, renamed, interrupted, canvas, node, earlier
        case earlierPrompts = "earlier_prompts"
    }
}

struct RunSummary: Decodable, Sendable, Hashable {
    let id: String
    let startedAt: Date?
    let stopping: Bool?
    let compact: Bool?
    /// What the agent does now.
    var activity: String?
    /// How many events the run published by the time the session was read:
    /// its stream, read from the first, has caught up at that one.
    var events: Int?

    enum CodingKeys: String, CodingKey {
        case id, stopping, compact, activity, events
        case startedAt = "started_at"
    }
}

/// One block of a transcript.
struct Entry: Codable, Sendable, Identifiable, Hashable {
    let id: String
    let kind: String
    var at: Date?
    var text: String?
    var detail: String?
    var phase: String?
    var state: String?
    var model: String?
    var forced: Bool?
    var tool: Tool?
    var images: [ImageInfo]?
    /// The preview the entry takes the place of: what streamed of it before
    /// the runner recorded it.
    var replaces: String?
    /// A preview gone void: the response it streamed of started over.
    var gone: Bool?
    /// A preview, the app's own: what streams of the agent's answer or
    /// thinking as it writes it, until the entry it becomes replaces it.
    var streaming: Bool?
    /// The row the entry is drawn in, the app's own: that of the preview it
    /// took the place of, so that the block stays as it was, open or not.
    var row: String?

    var rowID: String { row ?? id }

    static let user = "user", assistant = "assistant", reasoning = "reasoning", tool = "tool", notice = "notice", error = "error"

    struct ImageInfo: Codable, Sendable, Hashable {
        let label: String?
        var mediaType: String?
        var width: Int?
        var height: Int?
        var size: Int?

        /// The number of its label, "[Image 3]": how the server finds it.
        var number: Int? {
            guard let label, label.hasPrefix("[Image "), label.hasSuffix("]") else { return nil }
            return Int(label.dropFirst(7).dropLast())
        }

        enum CodingKeys: String, CodingKey {
            case label, width, height, size
            case mediaType = "media_type"
        }
    }

    /// The ID of the prompt's message, as the server's routes take it: a
    /// user entry is "input:<message>".
    var messageID: String? {
        guard kind == Entry.user, id.hasPrefix("input:") else { return nil }
        return String(id.dropFirst(6))
    }
}

struct Tool: Codable, Sendable, Hashable {
    let callID: String
    var name: String?
    var input: String?
    var state: String
    var exitCode: Int?
    var output: String?
    var stderr: String?
    var error: String?
    var started: Date?
    var finished: Date?
    var files: [String]?
    var calls: [CodeCall]?
    var logs: String?
    var value: String?
    /// The picture a ViewImage call read, as the model got it; its bytes
    /// stay with the call (…/tools/{call}/image).
    var image: Entry.ImageInfo?
    /// What a Write call wrote, or an Edit call changed.
    var change: Change?

    var isFinished: Bool { ["done", "failed", "canceled"].contains(state) }

    enum CodingKeys: String, CodingKey {
        case name, input, state, output, stderr, error, started, finished, files, calls, logs, value, image, change
        case callID = "call_id", exitCode = "exit_code"
    }

    /// What a Write call wrote, or an Edit call changed, as the cockpit
    /// shows it (FileView in cmd/internal/cockpit): the code in the colours
    /// of its tokens.
    struct Change: Codable, Sendable, Hashable {
        /// "write" or "edit".
        let kind: String
        /// What a Write call wrote, whole.
        var content: String?
        /// What an Edit call changed: the hunks of a unified diff, without
        /// the heads of the files — a line is a hunk's head ("@@ -12,3
        /// +12,4 @@"), or a sign ("-", "+" or " ") and a line of code.
        var diff: String?
        /// The line of the file the change starts at, and the hunks number
        /// the file's lines; nil when the edit did not say (it failed), and
        /// they number the strings'.
        var line: Int?
        /// The runs of the colours of content, or of diff: pairs of a
        /// length, in UTF-16 units, and the index of a class in the
        /// cockpit's syntax_classes. Signs and hunks' heads are plain.
        var syntax: [Int32]?
        /// How many times an Edit call made its change, when more than once.
        var replacements: Int?

        /// What there is to show: what a Write call wrote, or an Edit call's
        /// diff; nil when nothing.
        var text: String? {
            switch kind {
            case "write": content ?? ""
            case "edit": diff.flatMap { $0.isEmpty ? nil : $0 }
            default: nil
            }
        }

        /// How many lines it has: a newline at its end starts no line.
        var lineCount: Int {
            guard let text, !text.isEmpty else { return 0 }
            var lines = 1
            for unit in text.utf16 where unit == 0x0A { lines += 1 }
            return text.utf16.last == 0x0A ? lines - 1 : lines
        }

        /// The lines an Edit call put in and took out.
        var counts: (added: Int, removed: Int) {
            var added = 0, removed = 0
            var start = true
            for unit in (kind == "edit" ? diff ?? "" : "").utf16 {
                if start {
                    if unit == 0x2B { added += 1 } else if unit == 0x2D { removed += 1 }
                }
                start = unit == 0x0A
            }
            return (added, removed)
        }
    }
}

struct CodeCall: Codable, Sendable, Hashable {
    let name: String
    var input: String?
    var gist: String?
    var output: String?
    var error: String?
    var exitCode: Int?
    var state: String

    enum CodingKeys: String, CodingKey {
        case name, input, gist, output, error, state
        case exitCode = "exit_code"
    }
}

struct Usage: Decodable, Sendable, Hashable {
    let input: Int64?
    let cached: Int64?
    let output: Int64?
    let reasoning: Int64?
    let context: Int64?
    let turns: Int?
}

struct Queue: Decodable, Sendable, Hashable {
    let items: [QueueItem]?
    let paused: Bool?
}

struct QueueItem: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let text: String
    let at: Date?
    let forced: Bool?
    let model: String?
    let images: [Entry.ImageInfo]?
}

/// An event of a run, as /api/runs/{id}/events streams them.
struct RunEvent: Decodable, Sendable {
    let type: String
    let entry: Entry?
    let usage: Usage?
    let activity: String?
    let stopping: Bool?
    let text: String?
    let error: String?
    let stopped: Bool?
    let interrupted: Bool?
    let queue: Queue?
    let next: RunSummary?
    /// A "delta": a piece of what the agent writes, as it writes it.
    let delta: StreamDelta?
    /// An "output": what a running command printed.
    let output: StreamOutput?
}

/// StreamDelta is a piece of the agent's answer or thinking as it streams,
/// for a preview the entry it becomes replaces (Entry.replaces).
struct StreamDelta: Decodable, Sendable {
    /// The preview's: "stream:<turn>.<n>".
    let id: String
    /// Entry.assistant or Entry.reasoning.
    let kind: String
    /// When the preview began.
    let at: Date?
    /// What the preview gains.
    let text: String
    /// Whether the text is of a compaction's summary, which no entry
    /// replaces: the compaction's notice holds it once it is done.
    let compaction: Bool?
}

/// StreamOutput is what the command of a running tool call printed.
struct StreamOutput: Decodable, Sendable {
    /// The tool call's entry.
    let id: String
    /// "out" or "err".
    let stream: String
    let text: String
    /// How much it printed before text that is not shown, in bytes: it
    /// printed faster than it was read.
    let skipped: Int64?
}

/// LiveOutput is what a running command printed so far, as it is shown
/// until the call's entry has its output: the end of it.
struct LiveOutput: Equatable, Sendable {
    var out = ""
    var err = ""
    /// Both, in the order they came.
    var all = ""
    /// Bytes not shown: printed faster than they were read.
    var skipped: Int64 = 0
    /// Whether only the end of it is kept.
    var cut = false
    /// The lines it printed, those not kept too: the lines shown are known
    /// by their number.
    var lines = 0

    static let keep = 64 * 1024

    mutating func add(_ output: StreamOutput) {
        if let skipped = output.skipped, skipped > 0 { self.skipped += skipped }
        if output.stream == "err" {
            err = keepEnd(err + output.text)
        } else {
            out = keepEnd(out + output.text)
        }
        all = keepEnd(all + output.text)
        lines += output.text.utf8.reduce(0) { $1 == 10 ? $0 + 1 : $0 }
    }

    /// keepEnd keeps the last of text, from the start of a line.
    private mutating func keepEnd(_ text: String) -> String {
        let bytes = text.utf8
        guard bytes.count > Self.keep else { return text }
        cut = true
        var start = bytes.index(bytes.endIndex, offsetBy: -Self.keep)
        if let newline = bytes[start...].firstIndex(of: 10), bytes.distance(from: start, to: newline) < 4096 {
            start = bytes.index(after: newline)
        } else {
            // Not in the middle of a character.
            while start < bytes.endIndex, bytes[start] & 0xC0 == 0x80 { start = bytes.index(after: start) }
        }
        return String(decoding: bytes[start...], as: UTF8.self)
    }
}

struct StartRunRequest: Encodable, Sendable {
    let prompt: String
    let sessionID: String
    let messageID: String
    let resume: Bool
    /// The model to run with; nil for the connection's.
    var model: String?
    /// The effort; nil for the one saved in the cockpit.
    var thinkingLevel: String?
    var images: [PromptImage]?
    /// The message ID of the session's prompt this one replaces: the
    /// session goes back to before it, and this prompt runs from there.
    var rewind: String?

    enum CodingKeys: String, CodingKey {
        case prompt, resume, model, images, rewind
        case sessionID = "session_id", messageID = "message_id", thinkingLevel = "thinking_level"
    }
}

/// An image that goes with a prompt, whose text refers to it by its label.
struct PromptImage: Encodable, Sendable, Hashable {
    let label: String
    let mediaType: String
    /// The image's bytes, in base64: Go reads []byte so.
    let data: String

    enum CodingKeys: String, CodingKey {
        case label, data
        case mediaType = "media_type"
    }
}

struct StartRunResponse: Decodable, Sendable {
    let runID: String
    let sessionID: String
    let messageID: String?

    enum CodingKeys: String, CodingKey {
        case runID = "run_id", sessionID = "session_id", messageID = "message_id"
    }
}

struct EnqueueRequest: Encodable, Sendable {
    let text: String
    let force: Bool
    var model: String?
    var images: [PromptImage]?
}

/// A change to a queued message (PATCH …/queue/{item}); the server takes no
/// other keys.
struct QueueChange: Encodable, Sendable {
    var text: String?
    var force = false
    var position: Int?
    var model: String?
}

/// A change to a session (PATCH …/sessions/{id}).
struct SessionChange: Encodable, Sendable {
    var title: String?
    var pinned: Bool?
}

struct SessionChanged: Decodable, Sendable {
    let id: String
    let title: String?
    let pinned: Bool?
}

struct BranchRequest: Encodable, Sendable {
    /// The prompt to branch before; "" duplicates the whole session.
    let messageID: String

    enum CodingKeys: String, CodingKey {
        case messageID = "message_id"
    }
}

struct BranchResponse: Decodable, Sendable {
    /// The new session, when the branch has history; else a new session
    /// starts with the prompt.
    let sessionID: String?
    let prompt: String?

    enum CodingKeys: String, CodingKey {
        case prompt
        case sessionID = "session_id"
    }
}

struct EnqueueResponse: Decodable, Sendable {
    let run: RunSummary?
    let item: QueueItem?
    let queue: Queue?
    let note: String?
}

struct Empty: Codable, Sendable {}

// MARK: Canvases

/// GET …/canvases.
struct CanvasListResponse: Decodable, Sendable {
    let canvases: [CanvasSummary]
}

struct CanvasSummary: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let title: String
    let live: Bool?
    let updatedAt: Date?
    let createdAt: Date?
    let nodes: Int?
    let agents: Int?
    let terminals: Int?
    let busy: Int?
    let waiting: Int?
    let loaded: Bool?
    let readOnly: Bool?

    enum CodingKeys: String, CodingKey {
        case id, title, live, nodes, agents, terminals, busy, waiting, loaded
        case updatedAt = "updated_at", createdAt = "created_at", readOnly = "read_only"
    }
}

struct CanvasSnapshot: Decodable, Sendable {
    let doc: CanvasDoc
    let readOnly: Bool?
    let status: [String: NodeStatus]?
    let pending: [String: Int]?
    /// The messages that wait: for their target, or for the user's approval.
    let queue: [CanvasMessage]?
    let messages: [CanvasMessage]?
    /// The addresses of the canvas's webhooks, by node.
    let hooks: [String: String]?

    enum CodingKeys: String, CodingKey {
        case doc, status, pending, queue, messages, hooks
        case readOnly = "read_only"
    }
}

struct CanvasDoc: Decodable, Sendable, Hashable {
    let id: String
    var title: String
    var rev: Int64?
    var live: Bool?
    /// How far its agents go — autonomy and routing —, kept as the cockpit
    /// wrote them: what is changed goes back whole.
    var settings: JSONValue?
    var nodes: [CanvasNode]?
    var edges: [CanvasEdge]?
}

struct CanvasNode: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let kind: String
    let preset: String?
    let plugin: String?
    let title: String
    let x: Double
    let y: Double
    let w: Double
    let h: Double
    let z: Int?
    let config: JSONValue?
    /// What it may do to the canvas: none, observe, talk, build or admin.
    let access: String?
    /// Who made it: "user", or the node of the agent that did.
    let createdBy: String?
    let depth: Int?
    /// Made by an agent, it waits for the user's approval: nothing runs
    /// for it till then.
    let proposed: Bool?
    var runtime: NodeRuntime?

    enum CodingKeys: String, CodingKey {
        case id, kind, preset, plugin, title, x, y, w, h, z, config, access, depth, proposed, runtime
        case createdBy = "created_by"
    }

    static let terminal = "terminal", agent = "agent", source = "source", note = "note"
    /// The preset of the agent that builds the canvas.
    static let foreman = "foreman"

    var frame: CGRect { CGRect(x: x, y: y, width: max(w, 60), height: max(h, 40)) }
    var isProposed: Bool { proposed == true }
    /// A kou agent's session: reserved when the node is made, written from
    /// its first run on.
    var session: String? {
        guard kind == Self.agent, let session = runtime?.session, !session.isEmpty else { return nil }
        return session
    }
}

struct NodeRuntime: Decodable, Sendable, Hashable {
    let terminal: String?
    let session: String?
    /// The session was the user's before it came on the canvas.
    let adopted: Bool?
    let agentSession: String?
    let worktree: String?
    let branch: String?
    /// The generation of its token: rotating it voids the one before.
    let epoch: Int?
    /// Its agent was told of the canvas.
    let briefed: Bool?

    enum CodingKeys: String, CodingKey {
        case terminal, session, adopted, worktree, branch, epoch, briefed
        case agentSession = "agent_session"
    }
}

struct CanvasEdge: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let from: Port
    let to: Port
    /// What it makes of an output; {{text}} when empty.
    let template: String?
    /// auto, approve — each message waits for the user — or off.
    let mode: String?
    /// queue — a message waits while its target is busy — or now.
    let deliver: String?
    /// Whether a line naming where a message comes from goes first; nil:
    /// as the target wants.
    let header: Bool?

    static let auto = "auto", approve = "approve", off = "off"
    static let queue = "queue", now = "now"

    var effectiveMode: String { mode.flatMap { $0.isEmpty ? nil : $0 } ?? Self.auto }
    var effectiveDeliver: String { deliver.flatMap { $0.isEmpty ? nil : $0 } ?? Self.queue }
}

struct Port: Decodable, Sendable, Hashable {
    let node: String
    let port: String

    init(node: String, port: String) {
        self.node = node
        self.port = port
    }

    // What the user sent comes from no node.
    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        node = try container.decodeIfPresent(String.self, forKey: .node) ?? ""
        port = try container.decodeIfPresent(String.self, forKey: .port) ?? ""
    }

    enum CodingKeys: String, CodingKey {
        case node, port
    }
}

struct NodeStatus: Decodable, Sendable, Hashable {
    let state: String
    let detail: String?
    let since: Date?
    let activity: String?
    /// The agent seen running in a terminal: what is sent to it goes as a
    /// prompt.
    let agent: DetectedAgent?
    let quiet: Bool?
}

struct DetectedAgent: Decodable, Sendable, Hashable {
    let id: String?
    let title: String?
    let detected: Bool?
}

struct CanvasMessage: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let chain: String?
    let hops: Int?
    let from: Port?
    let edge: String?
    let to: Port?
    let at: Date?
    let title: String?
    let text: String?
    var state: String?
    let reason: String?
    let deliver: String?
    let deliveredAt: Date?
    /// Only its first part came: the message itself, read alone, is whole.
    let truncated: Bool?

    enum CodingKeys: String, CodingKey {
        case id, chain, hops, from, edge, to, at, title, text, state, reason, deliver, truncated
        case deliveredAt = "delivered_at"
    }

    static let pending = "pending", awaiting = "awaiting_approval", delivered = "delivered", dropped = "dropped"

    /// Whether it waits: for its target, or for the user.
    var isWaiting: Bool { state == Self.pending || state == Self.awaiting }
    var awaitsApproval: Bool { state == Self.awaiting }

    /// Its text, without the line that names where it comes from.
    var content: String {
        let text = self.text ?? ""
        guard text.hasPrefix("[canvas] "), let newline = text.firstIndex(of: "\n") else { return text }
        let head = text[..<newline]
        guard head.hasSuffix(":"), head.contains("from «") else { return text }
        return String(text[text.index(after: newline)...])
    }
}

/// An event of a canvas, as …/canvases/{id}/events streams them; a
/// snapshot is read as a CanvasSnapshot.
struct CanvasEvent: Decodable, Sendable {
    let type: String
    let node: String?
    let edge: String?
    let status: NodeStatus?
    let runtime: NodeRuntime?
    let entry: FeedEntry?
    let message: CanvasMessage?
    let text: String?
    let preview: String?
    let title: String?
    let port: String?
    let level: String?
    let at: Date?
    /// ops: the canvas's revision after them, who made them, and what they
    /// changed.
    let rev: Int64?
    let actor: CanvasActor?
    let changes: [CanvasChange]?
}

struct CanvasActor: Decodable, Sendable, Hashable {
    /// user, node or system.
    let kind: String?
    let id: String?
}

/// What a batch changed: a node or an edge, whole, or the ID of one
/// removed.
struct CanvasChange: Decodable, Sendable {
    let op: String
    let id: String?
    let node: CanvasNode?
    let edge: CanvasEdge?
    let canvas: CanvasChangeSet?
}

struct CanvasChangeSet: Decodable, Sendable {
    let title: String?
    let live: Bool?
    let settings: JSONValue?
}

/// A line of an agent node's feed: a transcript entry, cut short.
struct FeedEntry: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let kind: String
    let phase: String?
    let at: Date?
    let text: String?
    let tool: FeedTool?
    let truncated: Bool?
}

struct FeedTool: Decodable, Sendable, Hashable {
    let name: String
    let state: String
    let summary: String?
    let exitCode: Int?

    enum CodingKeys: String, CodingKey {
        case name, state, summary
        case exitCode = "exit_code"
    }
}

struct NodeTranscript: Decodable, Sendable {
    let session: String?
    let exists: Bool?
    let running: Bool?
    let entries: [FeedEntry]
}

struct NodeRead: Decodable, Sendable {
    let text: String
    let status: NodeStatus?
}

struct NodeSendRequest: Encodable, Sendable {
    var text: String?
    var submit: Bool?
    var keys: [String]?
    /// queue: it waits while the node is busy; now: it goes at once.
    var deliver: String?
}

struct RestartRequest: Encodable, Sendable {
    let resume: Bool
}

/// POST …/canvases/{id}/ops: operations that apply whole or not at all.
struct CanvasBatch: Encodable, Sendable {
    let ops: [JSONValue]
}

struct CanvasBatchResult: Decodable, Sendable {
    let rev: Int64?
    /// The IDs of what the batch made, by the index of its operation.
    let ids: [String: String]?
}

/// POST …/canvases.
struct CreateCanvasRequest: Encodable, Sendable {
    let title: String
    let template: String?
}

struct CreateCanvasResponse: Decodable, Sendable {
    let canvas: CanvasSummary?
}

/// PATCH …/canvases/{id}.
struct CanvasPatch: Encodable, Sendable {
    var title: String?
    var live: Bool?
}

/// GET …/canvas/templates.
struct CanvasTemplateList: Decodable, Sendable {
    let templates: [CanvasTemplate]
}

struct CanvasTemplate: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let title: String?
    let description: String?
    let nodes: Int?
    let edges: Int?
    let kinds: [String]?

    var name: String { title.flatMap { $0.isEmpty ? nil : $0 } ?? id }
}

/// POST …/messages/{id}/approve: with text, the message goes as edited.
struct ApproveRequest: Encodable, Sendable {
    let text: String?
}

struct MessageAnswer: Decodable, Sendable {
    let message: CanvasMessage?
}

/// POST …/nodes/{id}/fire.
struct FireRequest: Encodable, Sendable {
    let text: String
}

/// POST …/canvases/{id}/foreman.
struct ForemanRequest: Encodable, Sendable {
    let text: String
}

struct ForemanAnswer: Decodable, Sendable {
    let node: String?
    let created: Bool?
}

/// POST …/canvases/{id}/template.
struct SaveTemplateRequest: Encodable, Sendable {
    let name: String
    let description: String
}

/// GET …/canvas/kinds: what nodes can be.
struct CanvasKinds: Decodable, Sendable {
    let harnesses: [HarnessKind]?
    let sources: [SourceKind]?
    let agent: AgentKind?
}

/// A terminal's preset: a shell, a command, or a coding agent's program.
struct HarnessKind: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let plugin: String?
    let title: String?
    let icon: String?
    let description: String?
    let config: ConfigSchema?
    let launch: String?
    let installed: Bool?

    var name: String { (title.flatMap { $0.isEmpty ? nil : $0 } ?? id).replacingOccurrences(of: "…", with: "") }
}

struct SourceKind: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let plugin: String?
    let title: String?
    let icon: String?
    let description: String?
    let config: ConfigSchema?
    let outputs: [KindPort]?
    let builtin: Bool?

    var name: String { title.flatMap { $0.isEmpty ? nil : $0 } ?? id }
}

struct KindPort: Decodable, Sendable, Hashable {
    let id: String
    let title: String?
}

struct AgentKind: Decodable, Sendable {
    let config: ConfigSchema?
}

/// The JSON schema of a node's configuration: an object of strings (with
/// enum, a choice), numbers, booleans and lists of strings.
struct ConfigSchema: Decodable, Sendable, Hashable {
    let required: [String]?
    let properties: [String: SchemaProperty]?

    /// Its fields: those required first, then by title. The cockpit sends
    /// them in no order.
    var fields: [SchemaField] {
        let required = Set(self.required ?? [])
        return (properties ?? [:]).map { SchemaField(key: $0.key, property: $0.value, required: required.contains($0.key)) }
            .sorted { ($0.required ? 0 : 1, $0.title.lowercased()) < ($1.required ? 0 : 1, $1.title.lowercased()) }
    }
}

struct SchemaField: Hashable, Sendable, Identifiable {
    let key: String
    let property: SchemaProperty
    let required: Bool

    var id: String { key }
    var title: String { property.title.flatMap { $0.isEmpty ? nil : $0 } ?? key }
}

struct SchemaProperty: Decodable, Sendable, Hashable {
    let type: String?
    let title: String?
    let description: String?
    let options: [JSONValue]?
    let defaultValue: JSONValue?
    let pattern: String?

    enum CodingKeys: String, CodingKey {
        case type, title, description, pattern
        case options = "enum", defaultValue = "default"
    }

    var choices: [String] { options?.compactMap(\.text) ?? [] }
}

// MARK: The tunnel

struct RelayHealth: Decodable, Sendable, Hashable {
    let relay: String
    let proto: Int
    let authorized: Bool?
    let version: String?
    let agent: Bool?
    let agentSince: Date?

    enum CodingKeys: String, CodingKey {
        case relay, authorized, version, agent
        case proto = "protocol", agentSince = "agent_since"
    }
}

/// The cockpit's word on its tunnel (GET /api/tunnel).
struct TunnelStatus: Decodable, Sendable, Hashable {
    let state: String
    let wanted: Bool?
    let url: String?
    let host: String?
    let since: Date?
    let error: String?
    let remote: Bool?
}

// MARK: JSON of any shape

/// A node's configuration, whose shape is its kind's.
enum JSONValue: Codable, Sendable, Hashable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case let .string(value): try container.encode(value)
        // A whole number goes as one: the cockpit reads its counts as ints.
        case let .number(value) where value.rounded() == value && abs(value) < 1e15: try container.encode(Int64(value))
        case let .number(value): try container.encode(value)
        case let .bool(value): try container.encode(value)
        case let .object(value): try container.encode(value)
        case let .array(value): try container.encode(value)
        case .null: try container.encodeNil()
        }
    }

    subscript(key: String) -> JSONValue? {
        if case let .object(object) = self { return object[key] }
        return nil
    }

    var string: String? {
        if case let .string(value) = self { return value }
        return nil
    }

    var bool: Bool? {
        if case let .bool(value) = self { return value }
        return nil
    }

    var number: Double? {
        if case let .number(value) = self { return value }
        return nil
    }

    var object: [String: JSONValue]? {
        if case let .object(value) = self { return value }
        return nil
    }

    /// The value as a field shows it: a string, a number or a boolean.
    var text: String? {
        switch self {
        case let .string(value): value
        case let .number(value): value.rounded() == value && abs(value) < 1e15 ? String(Int64(value)) : String(value)
        case let .bool(value): value ? "true" : "false"
        default: nil
        }
    }
}

extension JSONValue: ExpressibleByStringLiteral, ExpressibleByBooleanLiteral, ExpressibleByIntegerLiteral,
    ExpressibleByDictionaryLiteral, ExpressibleByArrayLiteral
{
    init(stringLiteral value: String) { self = .string(value) }
    init(booleanLiteral value: Bool) { self = .bool(value) }
    init(integerLiteral value: Int) { self = .number(Double(value)) }
    init(dictionaryLiteral elements: (String, JSONValue)...) {
        self = .object(Dictionary(elements, uniquingKeysWith: { _, last in last }))
    }
    init(arrayLiteral elements: JSONValue...) { self = .array(elements) }
}
