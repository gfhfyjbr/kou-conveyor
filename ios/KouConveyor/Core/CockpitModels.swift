import Foundation

// The rest of the cockpit's API that the page uses: its settings
// (/api/config, /api/preferences), the models a workspace reaches, the
// accounts gateway (/api/accounts, /api/usage), the workspace's files, and
// what a prompt changed. Keys are spelled out, as in Models.swift.

// MARK: Effort and preferences

/// What /api/config says that the app uses: the effort and its levels, and
/// the model of the connection.
struct CockpitConfig: Decodable, Sendable {
    let thinking: String?
    let thinkingLevels: [String]?
    let effortSaved: Bool?
    let model: String?
    let provider: String?

    enum CodingKeys: String, CodingKey {
        case thinking, model, provider
        case thinkingLevels = "thinking_levels", effortSaved = "effort_saved"
    }
}

/// What runs go with unless a prompt says otherwise. The cockpit keeps them
/// where the terminal cockpit reads them too.
struct Preferences: Decodable, Sendable {
    let effort: String?
    let saved: Bool?
    let toolProfile: String?
    let toolProfiles: [Choice]?
    let sandbox: String?
    let sandboxes: [Choice]?

    struct Choice: Decodable, Sendable, Hashable, Identifiable {
        let value: String
        let description: String?

        var id: String { value }
    }

    enum CodingKeys: String, CodingKey {
        case effort, saved, sandbox, sandboxes
        case toolProfile = "tool_profile", toolProfiles = "tool_profiles"
    }
}

/// A preference to save (PUT /api/preferences): one at a time, as the
/// cockpit's page sends them.
struct PreferencesChange: Encodable, Sendable {
    var effort: String?
    var toolProfile: String?
    var sandbox: String?

    enum CodingKeys: String, CodingKey {
        case effort, sandbox
        case toolProfile = "tool_profile"
    }
}

// MARK: Models

/// The models a workspace's connection reaches: through the accounts
/// gateway, every model of every account (GET /api/w/{ws}/models).
struct ModelCatalog: Decodable, Sendable {
    let source: String?
    let defaultModel: String?
    let models: [ModelInfo]?
    let error: String?

    enum CodingKeys: String, CodingKey {
        case source, models, error
        case defaultModel = "default"
    }

    func find(_ id: String?) -> ModelInfo? {
        guard let id, !id.isEmpty else { return nil }
        return models?.first { $0.id == id }
    }
}

struct ModelInfo: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let name: String?
    let provider: String?
    let providerName: String?
    let api: String?
    /// The tokens its context window holds.
    let context: Int?
    /// It makes pictures or sound rather than answers.
    let media: Bool?
    /// Every account that serves it waits out a limit.
    let cooling: Bool?
    /// The accounts that serve it.
    let via: [String]?

    var title: String {
        if let name, !name.isEmpty { return name }
        return id
    }

    enum CodingKeys: String, CodingKey {
        case id, name, provider, api, context, media, cooling, via
        case providerName = "provider_name"
    }
}

// MARK: Accounts

/// What the cockpit's page shows of an amount of money: dollars, or the
/// user's own credits, each worth usd dollars.
struct MoneyUnit: Decodable, Sendable, Hashable {
    let name: String
    let usd: Double?
}

/// GET /api/accounts: the accounts the gateway serves models with, and how
/// each did over the range.
struct AccountsView: Decodable, Sendable {
    let range: String?
    let accounts: [Account]?
    let endpoints: [Endpoint]?
    let gateway: GatewayView?
    let connection: ConnectionView?
    let unit: MoneyUnit?
    let error: String?
}

struct GatewayView: Decodable, Sendable {
    /// running, starting, failed… or "off" when the cockpit runs without it.
    let state: String?
    let error: String?
    let since: Date?
    let address: String?
    let url: String?
    let version: String?
    let summary: GatewaySummary?
    let enabled: Bool?
    let use: Use?

    struct Use: Decodable, Sendable {
        let api: String?
        let model: String?
    }

    var isOff: Bool { state == nil || state == "off" }
}

struct GatewaySummary: Decodable, Sendable {
    let accounts: Int?
    let keys: Int?
    let endpoints: Int?
    let failingEndpoints: Int?
    let ready: Int?
    let cooling: Int?
    let failing: Int?
    let disabled: Int?
    let refreshing: Int?
    let ok: Int64?
    let failed: Int64?
    let cost: Double?
    let unpricedTokens: Int64?

    enum CodingKeys: String, CodingKey {
        case accounts, keys, endpoints, ready, cooling, failing, disabled, refreshing, ok, failed, cost
        case failingEndpoints = "failing_endpoints", unpricedTokens = "unpriced_tokens"
    }
}

/// How the cockpit reaches models: by the gateway, or straight.
struct ConnectionView: Decodable, Sendable {
    let mode: String?
    let model: String?
    let protocolName: String?
    let api: String?
    let baseURL: String?
    let viaGateway: Bool?
    let available: Bool?
    let error: String?

    enum CodingKeys: String, CodingKey {
        case mode, model, api, available, error
        case protocolName = "protocol", baseURL = "base_url", viaGateway = "via_gateway"
    }
}

struct Account: Decodable, Sendable, Identifiable {
    let name: String
    let id: String
    let provider: String?
    let providerName: String?
    let label: String?
    let plan: String?
    let kind: String?
    /// ready, cooling, error, disabled, refreshing, configured.
    let state: String?
    let message: String?
    let retryAt: Date?
    let created: Date?
    let refreshed: Date?
    let priority: Int?
    let note: String?
    let cooldowns: [Cooldown]?
    let uptime: Uptime?
    let spend: Spend?
    let errors: [ErrorSample]?
    let quota: Quota?
    let quotaSupported: Bool?
    let models: [String]?

    var title: String {
        if let label, !label.isEmpty { return label }
        return name
    }

    enum CodingKeys: String, CodingKey {
        case name, id, provider, label, plan, kind, state, message, created, refreshed, priority, note
        case cooldowns, uptime, spend, errors, quota, models
        case providerName = "provider_name", retryAt = "retry_at", quotaSupported = "quota_supported"
    }
}

/// A model of an account waiting out a limit.
struct Cooldown: Decodable, Sendable {
    let model: String?
    let reason: String?
    let until: Date?
    let status: Int?
}

struct ErrorSample: Decodable, Sendable {
    let at: Date?
    let model: String?
    let status: Int?
    let message: String?
}

/// What is left of an account's rate limits.
struct Quota: Decodable, Sendable {
    let source: String?
    let at: Date?
    let plan: String?
    let windows: [QuotaWindow]?
    let notes: [String]?
    let error: String?
    let status: Int?
}

struct QuotaWindow: Decodable, Sendable {
    let label: String
    /// Percent used, 0 to 100.
    let used: Double?
    let resetsAt: Date?
    let amount: String?
    let reached: Bool?

    enum CodingKeys: String, CodingKey {
        case label, used, amount, reached
        case resetsAt = "resets_at"
    }
}

/// How the requests an account served went, slot by slot.
struct Uptime: Decodable, Sendable {
    let from: Date?
    let slotSeconds: Int?
    let slots: [Slot]?
    let ok: Int64?
    let failed: Int64?
    let inputTokens: Int64?
    let outputTokens: Int64?
    let avgLatencyMS: Double?
    let lastOK: Date?
    let lastFailure: Date?

    struct Slot: Decodable, Sendable {
        let ok: Int64?
        let failed: Int64?
    }

    enum CodingKeys: String, CodingKey {
        case from, slots, ok, failed
        case slotSeconds = "slot_seconds", inputTokens = "input_tokens", outputTokens = "output_tokens"
        case avgLatencyMS = "avg_latency_ms", lastOK = "last_ok", lastFailure = "last_failure"
    }
}

/// What requests used, and what that comes to at API prices.
struct Spend: Decodable, Sendable {
    let requests: Int64?
    let tokens: TokenCounts?
    let cost: Double?
    let unpricedTokens: Int64?

    enum CodingKeys: String, CodingKey {
        case requests, tokens, cost
        case unpricedTokens = "unpriced_tokens"
    }
}

struct TokenCounts: Decodable, Sendable {
    let input: Int64?
    let cacheRead: Int64?
    let cacheWrite: Int64?
    let output: Int64?
    let reasoning: Int64?
    let other: Int64?

    /// As the gateway counts them: reasoning is in the output already.
    var total: Int64 {
        let parts: [Int64?] = [input, cacheRead, cacheWrite, output, other]
        return parts.reduce(0) { $0 + ($1 ?? 0) }
    }

    enum CodingKeys: String, CodingKey {
        case input, output, reasoning, other
        case cacheRead = "cache_read", cacheWrite = "cache_write"
    }
}

/// What tokens cost, in dollars, by their kind.
struct CostParts: Decodable, Sendable {
    let input: Double?
    let cacheRead: Double?
    let cacheWrite: Double?
    let output: Double?

    var total: Double {
        let parts: [Double?] = [input, cacheRead, cacheWrite, output]
        return parts.reduce(0) { $0 + ($1 ?? 0) }
    }

    enum CodingKeys: String, CodingKey {
        case input, output
        case cacheRead = "cache_read", cacheWrite = "cache_write"
    }
}

/// An endpoint of the gateway: an API with keys of the user's own.
struct Endpoint: Decodable, Sendable, Identifiable {
    let id: String
    let kind: String?
    let kindName: String?
    let name: String?
    let baseURL: String?
    let host: String?
    let keys: Int?
    let models: [Model]?
    let prefix: String?
    let disabled: Bool?
    let state: String?
    let uptime: Uptime?
    let spend: Spend?
    let errors: [ErrorSample]?

    struct Model: Decodable, Sendable {
        let name: String?
        let alias: String?
    }

    var title: String {
        if let name, !name.isEmpty { return name }
        return host ?? id
    }

    enum CodingKeys: String, CodingKey {
        case id, kind, name, host, keys, models, prefix, disabled, state, uptime, spend, errors
        case kindName = "kind_name", baseURL = "base_url"
    }
}

struct DisabledChange: Encodable, Sendable {
    let disabled: Bool
}

// MARK: Usage

/// GET /api/usage: what the gateway's requests used over a range, slot by
/// slot, and what that comes to at API prices.
struct UsageReport: Decodable, Sendable {
    let range: String?
    let zone: String?
    let from: Date?
    let to: Date?
    let starts: [Date]?
    let slotSeconds: Int?
    let since: Date?
    let unit: MoneyUnit?
    let totals: UsageTotals?
    let previous: UsageTotals?
    let rows: [UsageRow]?
    let models: [UsageModelInfo]?
    let accounts: [UsageAccount]?
    let facets: UsageFacets?
    let pricesError: String?

    enum CodingKeys: String, CodingKey {
        case range, zone, from, to, starts, since, unit, totals, previous, rows, models, accounts, facets
        case slotSeconds = "slot_seconds", pricesError = "prices_error"
    }

    func modelName(_ id: String) -> String {
        if let name = models?.first(where: { $0.id == id })?.name, !name.isEmpty { return name }
        return id
    }

    func accountName(_ id: String) -> String {
        guard let account = accounts?.first(where: { $0.id == id }) else { return id }
        if let label = account.label, !label.isEmpty { return label }
        return account.name ?? id
    }
}

struct UsageTotals: Decodable, Sendable {
    let requests: Int64?
    let failed: Int64?
    let tokens: TokenCounts?
    let cost: CostParts?
    let unpricedTokens: Int64?
    let cacheSaved: Double?
    let avgLatencyMS: Double?

    enum CodingKeys: String, CodingKey {
        case requests, failed, tokens, cost
        case unpricedTokens = "unpriced_tokens", cacheSaved = "cache_saved", avgLatencyMS = "avg_latency_ms"
    }
}

/// The requests of one slot, account and model.
struct UsageRow: Decodable, Sendable {
    /// Its slot's index in starts; absent for the first.
    let slot: Int?
    let account: String?
    let model: String?
    let requests: Int64?
    let failed: Int64?
    let milliseconds: Double?
    let tokens: TokenCounts?
    let cost: CostParts?

    enum CodingKeys: String, CodingKey {
        case slot = "s", account = "a", model = "m", requests = "n", failed = "f", milliseconds = "ms"
        case tokens = "t", cost = "c"
    }
}

struct UsageModelInfo: Decodable, Sendable {
    let id: String
    let name: String?
    let served: Bool?
}

struct UsageAccount: Decodable, Sendable {
    let id: String
    let name: String?
    let label: String?
    let provider: String?
    let providerName: String?
    let kind: String?
    let gone: Bool?

    enum CodingKeys: String, CodingKey {
        case id, name, label, provider, kind, gone
        case providerName = "provider_name"
    }
}

struct UsageFacets: Decodable, Sendable {
    let providers: [UsageFacet]?
    let accounts: [UsageFacet]?
    let models: [UsageFacet]?
}

struct UsageFacet: Decodable, Sendable, Identifiable {
    let id: String
    let requests: Int64?
    let tokens: Int64?
    let cost: Double?
    let unpricedTokens: Int64?

    enum CodingKeys: String, CodingKey {
        case id, requests, tokens, cost
        case unpricedTokens = "unpriced_tokens"
    }
}

// MARK: Files

/// GET /api/w/{ws}/tree?path=: a folder of the workspace.
struct TreeListing: Decodable, Sendable {
    let path: String?
    let entries: [TreeEntry]?
    let truncated: Bool?
}

struct TreeEntry: Decodable, Sendable, Identifiable, Hashable {
    let name: String
    let dir: Bool?
    let link: Bool?
    let broken: Bool?
    let size: Int64?
    let ignored: Bool?
    /// Its state in git: M, A, D, ?, R…
    let git: String?
    let changed: Bool?

    var id: String { name }
    var isDirectory: Bool { dir == true }
}

/// GET /api/w/{ws}/file?path=: a file's text and its highlighting, or that
/// it is a picture, binary or too large to show.
struct FileView: Decodable, Sendable {
    let path: String
    let size: Int64?
    let modified: Date?
    let version: String?
    /// The media type of a picture.
    let image: String?
    let tooLarge: Bool?
    let binary: Bool?
    let text: String?
    let language: String?
    /// Pairs of a length, in UTF-16 code units, and a class, by its index
    /// in classes.
    let runs: [Int32]?
    let complete: Bool?
    let classes: [String]?

    enum CodingKeys: String, CodingKey {
        case path, size, modified, version, image, binary, text, language, runs, complete, classes
        case tooLarge = "too_large"
    }
}

/// GET /api/w/{ws}/files?q=: the workspace's files that match.
struct FileSearch: Decodable, Sendable {
    let files: [FileMatch]?
}

struct FileMatch: Decodable, Sendable, Identifiable, Hashable {
    let path: String
    let directory: Bool?

    var id: String { path }
}

// MARK: Changes

/// What a prompt changed in the workspace (GET …/sessions/{id}/changes/{message}).
struct ChangesView: Decodable, Sendable {
    let message: String?
    let available: Bool?
    /// The prompt's run goes on: the changes grow.
    let live: Bool?
    let reason: String?
    let tree: String?
    let files: [FileChange]?
    let stat: Stat?

    struct Stat: Decodable, Sendable {
        let files: Int?
        let added: Int?
        let removed: Int?
    }
}

struct FileChange: Decodable, Sendable, Identifiable, Hashable {
    let path: String
    let oldPath: String?
    /// added, modified, deleted, renamed, typechange.
    let status: String?
    let added: Int?
    let removed: Int?
    let binary: Bool?

    var id: String { (oldPath ?? "") + "\u{0}" + path }

    enum CodingKeys: String, CodingKey {
        case path, status, added, removed, binary
        case oldPath = "old_path"
    }
}

struct DiffView: Decodable, Sendable {
    let path: String?
    let tree: String?
    let patch: String?
    let truncated: Bool?
}
