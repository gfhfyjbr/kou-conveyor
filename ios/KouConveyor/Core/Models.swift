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
}

struct RunSummary: Decodable, Sendable, Hashable {
    let id: String
    let startedAt: Date?
    let stopping: Bool?
    let compact: Bool?

    enum CodingKeys: String, CodingKey {
        case id, stopping, compact
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

    static let user = "user", assistant = "assistant", reasoning = "reasoning", tool = "tool", notice = "notice", error = "error"

    struct ImageInfo: Codable, Sendable, Hashable {
        let label: String?
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

    var isFinished: Bool { ["done", "failed", "canceled"].contains(state) }

    enum CodingKeys: String, CodingKey {
        case name, input, state, output, stderr, error, started, finished, files, calls, logs, value
        case callID = "call_id", exitCode = "exit_code"
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
}

struct StartRunRequest: Encodable, Sendable {
    let prompt: String
    let sessionID: String
    let messageID: String
    let resume: Bool

    enum CodingKeys: String, CodingKey {
        case prompt, resume
        case sessionID = "session_id", messageID = "message_id"
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
    let messages: [CanvasMessage]?

    enum CodingKeys: String, CodingKey {
        case doc, status, pending, messages
        case readOnly = "read_only"
    }
}

struct CanvasDoc: Decodable, Sendable, Hashable {
    let id: String
    let title: String
    let rev: Int64?
    let live: Bool?
    let nodes: [CanvasNode]?
    let edges: [CanvasEdge]?
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
    var runtime: NodeRuntime?

    static let terminal = "terminal", agent = "agent", source = "source", note = "note"

    var frame: CGRect { CGRect(x: x, y: y, width: max(w, 60), height: max(h, 40)) }
}

struct NodeRuntime: Decodable, Sendable, Hashable {
    let terminal: String?
    let session: String?
    let agentSession: String?
    let worktree: String?
    let branch: String?

    enum CodingKeys: String, CodingKey {
        case terminal, session, worktree, branch
        case agentSession = "agent_session"
    }
}

struct CanvasEdge: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let from: Port
    let to: Port
    let mode: String?
}

struct Port: Decodable, Sendable, Hashable {
    let node: String
    let port: String
}

struct NodeStatus: Decodable, Sendable, Hashable {
    let state: String
    let detail: String?
    let since: Date?
    let activity: String?
    let quiet: Bool?
}

struct CanvasMessage: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let from: Port?
    let to: Port?
    let at: Date?
    let title: String?
    let text: String?
    let state: String?
    let reason: String?
}

/// An event of a canvas, as …/canvases/{id}/events streams them; a
/// snapshot is read as a CanvasSnapshot.
struct CanvasEvent: Decodable, Sendable {
    let type: String
    let node: String?
    let status: NodeStatus?
    let runtime: NodeRuntime?
    let entry: FeedEntry?
    let message: CanvasMessage?
    let text: String?
    let preview: String?
    let title: String?
    let port: String?
    let at: Date?
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
}

struct RestartRequest: Encodable, Sendable {
    let resume: Bool
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
}
