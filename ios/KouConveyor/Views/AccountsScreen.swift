import Charts
import SwiftUI

// The Accounts tab: what the Accounts view of the cockpit's page shows —
// the accounts gateway, the accounts and endpoints it serves models with,
// their limits and how they did — and its Usage tab: what the requests
// used, and what that comes to at API prices.

// MARK: Models

/// AccountsModel is the gateway's accounts and endpoints, asked for again
/// every so often while they show.
@MainActor
@Observable
final class AccountsModel {
    let client: RelayClient
    /// The range the activity covers: 24h or 7d.
    var range = "24h"
    private(set) var view: AccountsView?
    private(set) var error: String?
    /// The limits asked for anew, by account.
    private(set) var quotas: [String: Quota] = [:]
    /// What is being done, by "quota:<name>", "account:<name>", "endpoint:<id>".
    private(set) var busy: Set<String> = []
    /// What went wrong doing something.
    var notice: String?

    init(client: RelayClient) {
        self.client = client
    }

    func load() async {
        do {
            let view: AccountsView = try await client.get("/api/accounts", query: [URLQueryItem(name: "range", value: range)])
            self.view = view
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = error.localizedDescription
        }
    }

    func poll() async {
        while !Task.isCancelled {
            await load()
            try? await Task.sleep(for: .seconds(20))
        }
    }

    func account(_ name: String) -> Account? {
        view?.accounts?.first { $0.name == name }
    }

    func endpoint(_ id: String) -> Endpoint? {
        view?.endpoints?.first { $0.id == id }
    }

    func quota(_ account: Account) -> Quota? {
        quotas[account.name] ?? account.quota
    }

    /// refreshQuota asks the provider for what is left of an account's limits.
    func refreshQuota(_ account: Account) async {
        let key = "quota:" + account.name
        busy.insert(key)
        defer { busy.remove(key) }
        do {
            let quota: Quota = try await client.get(API.path("api", "accounts", account.name, "quota"),
                                                    query: [URLQueryItem(name: "refresh", value: "1")])
            quotas[account.name] = quota
        } catch {
            notice = error.localizedDescription
        }
    }

    /// setDisabled turns an account off, so the gateway sends it no
    /// requests, or on again.
    func setDisabled(_ account: Account, _ disabled: Bool) async {
        await change("account:" + account.name, API.path("api", "accounts", account.name), disabled)
    }

    func setDisabled(_ endpoint: Endpoint, _ disabled: Bool) async {
        await change("endpoint:" + endpoint.id, API.path("api", "endpoints", endpoint.id), disabled)
    }

    private func change(_ key: String, _ path: String, _ disabled: Bool) async {
        busy.insert(key)
        defer { busy.remove(key) }
        do {
            try await client.send("PATCH", path, body: DisabledChange(disabled: disabled))
            await load()
        } catch {
            notice = error.localizedDescription
        }
    }
}

/// UsageModel is the report of the Usage tab, for a range and the filters
/// chosen.
@MainActor
@Observable
final class UsageModel {
    let client: RelayClient
    var range = "7d"
    var provider: String?
    var account: String?
    var model: String?
    private(set) var report: UsageReport?
    private(set) var error: String?
    private(set) var loading = false

    init(client: RelayClient) {
        self.client = client
    }

    /// The question the report answers: when it changes, it is asked again.
    var key: String { [range, provider ?? "", account ?? "", model ?? ""].joined(separator: "|") }
    var filtered: Bool { provider != nil || account != nil || model != nil }

    func load() async {
        loading = true
        defer { loading = false }
        let zone = TimeZone.current
        var query = [
            URLQueryItem(name: "range", value: range),
            URLQueryItem(name: "tz", value: zone.identifier),
            URLQueryItem(name: "offset", value: String(zone.secondsFromGMT() / 60)),
        ]
        if let provider { query.append(URLQueryItem(name: "provider", value: provider)) }
        if let account { query.append(URLQueryItem(name: "account", value: account)) }
        if let model { query.append(URLQueryItem(name: "model", value: model)) }
        do {
            let report: UsageReport = try await client.get("/api/usage", query: query)
            self.report = report
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = error.localizedDescription
        }
    }
}

// MARK: The tab

enum AccountRoute: Hashable {
    case account(String)
    case endpoint(String)
}

struct AccountsScreen: View {
    var body: some View {
        NavigationStack {
            CockpitGate { client, _ in
                AccountsRoot(client: client)
                    .id(ObjectIdentifier(client))
            }
        }
    }
}

struct AccountsRoot: View {
    enum Page: String, CaseIterable, Identifiable {
        case accounts = "Accounts", usage = "Usage"

        var id: String { rawValue }
    }

    @State private var page = Page.accounts
    @State private var accounts: AccountsModel
    @State private var usage: UsageModel

    init(client: RelayClient) {
        _accounts = State(initialValue: AccountsModel(client: client))
        _usage = State(initialValue: UsageModel(client: client))
    }

    var body: some View {
        Group {
            switch page {
            case .accounts: AccountsList(model: accounts)
            case .usage: UsageList(usage: usage, unit: accounts.view?.unit)
            }
        }
        .navigationTitle(page.rawValue)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                Picker("View", selection: $page) {
                    ForEach(Page.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .frame(width: 220)
            }
        }
        .navigationDestination(for: AccountRoute.self) { route in
            switch route {
            case let .account(name): AccountDetail(model: accounts, name: name)
            case let .endpoint(id): EndpointDetail(model: accounts, id: id)
            }
        }
    }
}

// MARK: Accounts

struct AccountsList: View {
    @Bindable var model: AccountsModel

    var body: some View {
        List {
            if let view = model.view {
                GatewaySection(gateway: view.gateway, unit: view.unit)
                let accounts = view.accounts ?? []
                let endpoints = view.endpoints ?? []
                if !accounts.isEmpty || !endpoints.isEmpty {
                    Section {
                        Picker("Range", selection: $model.range) {
                            Text("Last 24 hours").tag("24h")
                            Text("Last 7 days").tag("7d")
                        }
                        .pickerStyle(.segmented)
                    }
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
                }
                if !accounts.isEmpty {
                    Section("Accounts") {
                        ForEach(accounts) { account in
                            NavigationLink(value: AccountRoute.account(account.name)) {
                                AccountRow(account: account, quota: model.quota(account), unit: view.unit)
                            }
                        }
                    }
                }
                if !endpoints.isEmpty {
                    Section("Endpoints") {
                        ForEach(endpoints) { endpoint in
                            NavigationLink(value: AccountRoute.endpoint(endpoint.id)) {
                                EndpointRow(endpoint: endpoint, unit: view.unit)
                            }
                        }
                    }
                }
                ConnectionSection(connection: view.connection)
            }
        }
        .overlay {
            if model.view == nil {
                if let error = model.error {
                    ContentUnavailableView("Accounts unavailable", systemImage: "person.2.slash", description: Text(error))
                } else {
                    ProgressView()
                }
            }
        }
        .refreshable { await model.load() }
        .task { await model.poll() }
        .onChange(of: model.range) {
            Task { await model.load() }
        }
        .alert("Not done", isPresented: Binding(get: { model.notice != nil }, set: { if !$0 { model.notice = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.notice ?? "")
        }
    }
}

struct GatewaySection: View {
    let gateway: GatewayView?
    let unit: MoneyUnit?

    var body: some View {
        Section {
            if let gateway, !gateway.isOff {
                HStack(spacing: 10) {
                    Image(systemName: "point.3.filled.connected.trianglepath.dotted")
                        .foregroundStyle(Color.kouAccent)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Accounts gateway")
                            .font(.headline)
                        Text([gateway.version, gateway.url ?? gateway.address].compactMap { $0 }.joined(separator: " · "))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 4)
                    AccountStateBadge(state: gateway.state)
                }
                if let summary = gateway.summary {
                    HStack(spacing: 0) {
                        Tally(count: summary.ready, title: "Ready", color: .green)
                        Tally(count: summary.cooling, title: "Cooling", color: .orange)
                        Tally(count: summary.failing, title: "Failing", color: .red)
                        Tally(count: summary.disabled, title: "Off", color: .secondary)
                    }
                    if (summary.ok ?? 0) + (summary.failed ?? 0) > 0 {
                        LabeledContent("Requests", value: "\(Format.count(summary.ok)) ok · \(Format.count(summary.failed)) failed")
                    }
                    if summary.cost != nil {
                        LabeledContent("At API prices", value: Format.money(summary.cost, unit: unit))
                    }
                }
                if let model = gateway.use?.model, !model.isEmpty {
                    LabeledContent("Runs use", value: model)
                }
                if let since = gateway.since {
                    LabeledContent("Running since", value: since.formatted(.relative(presentation: .named)))
                }
                if let error = gateway.error, !error.isEmpty {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(.red)
                }
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    Label("The accounts gateway is off", systemImage: "powerplug")
                        .font(.headline)
                    Text(offText)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                .padding(.vertical, 4)
            }
        } header: {
            Text("Gateway")
        }
    }

    private var offText: String {
        let hint = "Start kou-conveyor-web with -accounts 127.0.0.1:8318 to serve models with the accounts you sign in with."
        if let error = gateway?.error, !error.isEmpty { return error + "\n" + hint }
        return "The cockpit runs without its accounts gateway. " + hint
    }
}

struct Tally: View {
    let count: Int?
    let title: String
    let color: Color

    var body: some View {
        VStack(spacing: 2) {
            Text("\(count ?? 0)")
                .font(.title3.weight(.semibold).monospacedDigit())
                .foregroundStyle((count ?? 0) > 0 ? color : Color.secondary)
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
    }
}

/// AccountStateBadge is the state of the gateway, an account or an endpoint.
struct AccountStateBadge: View {
    let state: String?

    var body: some View {
        Text(title)
            .font(.caption2.weight(.semibold))
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .foregroundStyle(color)
            .background(color.opacity(0.15), in: .capsule)
    }

    private var title: String {
        switch state ?? "" {
        case "ready": "Ready"
        case "cooling": "Cooling"
        case "error": "Error"
        case "disabled", "off": "Off"
        case "refreshing": "Refreshing"
        case "configured": "Configured"
        case "running": "Running"
        case "starting": "Starting"
        case "failed": "Failed"
        case "": "—"
        default: (state ?? "").capitalized
        }
    }

    private var color: Color {
        switch state ?? "" {
        case "ready", "running": .green
        case "cooling", "starting", "refreshing": .orange
        case "error", "failed": .red
        default: .secondary
        }
    }
}

/// ProviderMark is a provider's letter in its color.
struct ProviderMark: View {
    let provider: String?

    var body: some View {
        Text(letter)
            .font(.caption.weight(.bold))
            .foregroundStyle(.white)
            .frame(width: 26, height: 26)
            .background(color.gradient, in: .circle)
            .accessibilityHidden(true)
    }

    private var key: String { (provider ?? "").lowercased() }

    private var letter: String {
        key.first.map { String($0).uppercased() } ?? "?"
    }

    private var color: Color {
        if key.contains("claude") || key.contains("anthropic") { return .orange }
        if key.contains("codex") || key.contains("openai") { return .green }
        if key.contains("gemini") || key.contains("google") || key.contains("antigravity") { return .blue }
        if key.contains("xai") || key.contains("grok") { return .gray }
        if key.contains("qwen") || key.contains("iflow") { return .purple }
        return .kouAccent
    }
}

struct AccountRow: View {
    let account: Account
    let quota: Quota?
    let unit: MoneyUnit?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .center, spacing: 10) {
                ProviderMark(provider: account.provider)
                VStack(alignment: .leading, spacing: 2) {
                    Text(account.title)
                        .font(.body.weight(.medium))
                        .lineLimit(1)
                    Text([account.providerName, account.plan].compactMap { $0 }.joined(separator: " · "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                AccountStateBadge(state: account.state)
            }
            if let windows = quota?.windows, !windows.isEmpty {
                VStack(spacing: 6) {
                    ForEach(Array(windows.prefix(3).enumerated()), id: \.offset) { _, window in
                        QuotaBar(window: window, compact: true)
                    }
                }
            }
            if let slots = account.uptime?.slots, !slots.isEmpty {
                UptimeStrip(slots: slots)
                    .frame(height: 12)
            }
            HStack(spacing: 10) {
                if let spend = account.spend {
                    // The gateway leaves out what is zero.
                    Text("\(Format.count(spend.requests ?? 0)) req")
                    Text("\(Format.tokens(spend.tokens?.total)) tok")
                    Text(Format.money(spend.cost, unit: unit))
                }
                if let cooldowns = account.cooldowns, !cooldowns.isEmpty {
                    Label("\(cooldowns.count) cooling", systemImage: "snowflake")
                        .foregroundStyle(.orange)
                }
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
            if let message = account.message, !message.isEmpty {
                Text(message)
                    .font(.caption)
                    .foregroundStyle(account.state == "error" ? Color.red : Color.secondary)
                    .lineLimit(2)
            }
        }
        .padding(.vertical, 4)
    }
}

struct EndpointRow: View {
    let endpoint: Endpoint
    let unit: MoneyUnit?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                Image(systemName: "server.rack")
                    .foregroundStyle(Color.kouAccent)
                    .frame(width: 26)
                VStack(alignment: .leading, spacing: 2) {
                    Text(endpoint.title)
                        .font(.body.weight(.medium))
                        .lineLimit(1)
                    Text([endpoint.kindName, endpoint.host].compactMap { $0 }.joined(separator: " · "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                AccountStateBadge(state: endpoint.disabled == true ? "disabled" : endpoint.state)
            }
            if let slots = endpoint.uptime?.slots, !slots.isEmpty {
                UptimeStrip(slots: slots)
                    .frame(height: 12)
            }
            HStack(spacing: 10) {
                Text("\(endpoint.keys ?? 0) key\(endpoint.keys == 1 ? "" : "s")")
                Text("\(endpoint.models?.count ?? 0) models")
                if let spend = endpoint.spend {
                    Text("\(Format.count(spend.requests ?? 0)) req")
                    Text(Format.money(spend.cost, unit: unit))
                }
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
        }
        .padding(.vertical, 4)
    }
}

/// QuotaBar is one window of an account's limits: how much of it is used,
/// and when it starts anew.
struct QuotaBar: View {
    let window: QuotaWindow
    var compact = false

    var body: some View {
        let used = min(max(window.used ?? 0, 0), 100)
        VStack(alignment: .leading, spacing: 3) {
            HStack {
                Text(window.label)
                    .font(compact ? .caption2 : .subheadline)
                    .lineLimit(1)
                Spacer(minLength: 4)
                Text("\(Int(used.rounded()))%")
                    .font((compact ? Font.caption2 : Font.subheadline).monospacedDigit())
                    .foregroundStyle(tint)
            }
            ProgressView(value: used, total: 100)
                .tint(tint)
            if !compact {
                HStack {
                    if let amount = window.amount, !amount.isEmpty { Text(amount) }
                    Spacer(minLength: 4)
                    if let resets = window.resetsAt {
                        Text("Resets \(resets.formatted(.relative(presentation: .named)))")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
    }

    private var tint: Color {
        let used = window.used ?? 0
        if window.reached == true || used >= 90 { return .red }
        return used >= 70 ? .orange : .green
    }
}

/// UptimeStrip is how the requests of each slot went: green all served,
/// red all failed, orange some of each, grey none.
struct UptimeStrip: View {
    let slots: [Uptime.Slot]

    var body: some View {
        Canvas { context, size in
            let count = slots.count
            guard count > 0 else { return }
            let gap: CGFloat = count > 60 ? 0.5 : 1
            let width = max((size.width - gap * CGFloat(count - 1)) / CGFloat(count), 0.5)
            for (i, slot) in slots.enumerated() {
                let ok = slot.ok ?? 0, failed = slot.failed ?? 0
                let color: Color = ok + failed == 0 ? .gray.opacity(0.25) : failed == 0 ? .green : ok == 0 ? .red : .orange
                let rect = CGRect(x: CGFloat(i) * (width + gap), y: 0, width: width, height: size.height)
                context.fill(Path(roundedRect: rect, cornerRadius: min(2, width / 2)), with: .color(color))
            }
        }
        .accessibilityElement()
        .accessibilityLabel("Requests over time")
    }
}

struct ConnectionSection: View {
    let connection: ConnectionView?

    var body: some View {
        if let connection {
            Section {
                if let mode = connection.mode, !mode.isEmpty {
                    LabeledContent("Mode", value: mode)
                }
                if let model = connection.model, !model.isEmpty {
                    LabeledContent("Model", value: model)
                }
                if let api = connection.api ?? connection.protocolName, !api.isEmpty {
                    LabeledContent("API", value: api)
                }
                if let base = connection.baseURL, !base.isEmpty {
                    LabeledContent("Base URL") {
                        Text(base)
                            .font(.footnote.monospaced())
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                }
                LabeledContent("Through the gateway", value: connection.viaGateway == true ? "Yes" : "No")
                if let error = connection.error, !error.isEmpty {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(.red)
                }
            } header: {
                Text("Connection")
            } footer: {
                Text("How the cockpit's runs reach models.")
            }
        }
    }
}

// MARK: An account

struct AccountDetail: View {
    let model: AccountsModel
    let name: String
    @State private var showsModels = false

    var body: some View {
        if let account = model.account(name) {
            let quota = model.quota(account)
            let unit = model.view?.unit
            List {
                Section {
                    HStack(spacing: 12) {
                        ProviderMark(provider: account.provider)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(account.title)
                                .font(.headline)
                            Text([account.providerName, account.plan, account.kind].compactMap { $0 }.joined(separator: " · "))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 4)
                        AccountStateBadge(state: account.state)
                    }
                    if let message = account.message, !message.isEmpty {
                        Text(message)
                            .font(.footnote)
                            .foregroundStyle(account.state == "error" ? Color.red : Color.secondary)
                    }
                    if let retry = account.retryAt, retry > Date() {
                        LabeledContent("Tries again", value: retry.formatted(.relative(presentation: .named)))
                    }
                    Toggle("Serves requests", isOn: Binding(
                        get: { account.state != "disabled" },
                        set: { on in Task { await model.setDisabled(account, !on) } }
                    ))
                    .disabled(model.busy.contains("account:" + account.name))
                }

                Section {
                    if let quota {
                        ForEach(Array((quota.windows ?? []).enumerated()), id: \.offset) { _, window in
                            QuotaBar(window: window)
                                .padding(.vertical, 2)
                        }
                        ForEach(quota.notes ?? [], id: \.self) { note in
                            Text(note)
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                        }
                        if let error = quota.error, !error.isEmpty {
                            Text(error)
                                .font(.footnote)
                                .foregroundStyle(.red)
                        }
                    } else {
                        Text("The gateway has not read this account's limits yet.")
                            .foregroundStyle(.secondary)
                    }
                    if account.quotaSupported == true {
                        Button {
                            Task { await model.refreshQuota(account) }
                        } label: {
                            if model.busy.contains("quota:" + account.name) {
                                ProgressView()
                            } else {
                                Label("Read the limits now", systemImage: "arrow.clockwise")
                            }
                        }
                    }
                } header: {
                    Text("Limits")
                } footer: {
                    if let quota, let at = quota.at {
                        Text("\(quota.source == "live" ? "Asked" : "Seen in a response") \(at.formatted(.relative(presentation: .named))).")
                    }
                }

                if let cooldowns = account.cooldowns, !cooldowns.isEmpty {
                    Section("Cooling down") {
                        ForEach(Array(cooldowns.enumerated()), id: \.offset) { _, cooldown in
                            VStack(alignment: .leading, spacing: 3) {
                                Text(cooldown.model ?? "Every model")
                                    .font(.subheadline.weight(.medium))
                                HStack(spacing: 8) {
                                    if let reason = cooldown.reason, !reason.isEmpty { Text(reason) }
                                    if let status = cooldown.status, status > 0 { Text("HTTP \(status)") }
                                    Spacer(minLength: 0)
                                    if let until = cooldown.until {
                                        Text("until \(until.formatted(date: .omitted, time: .shortened))")
                                    }
                                }
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            }
                        }
                    }
                }

                if let uptime = account.uptime {
                    UptimeSection(uptime: uptime, range: model.range)
                }
                if let spend = account.spend {
                    SpendSection(spend: spend, unit: unit)
                }
                if let models = account.models, !models.isEmpty {
                    Section {
                        DisclosureGroup("\(models.count) model\(models.count == 1 ? "" : "s")", isExpanded: $showsModels) {
                            ForEach(models, id: \.self) { id in
                                Text(id)
                                    .font(.footnote.monospaced())
                                    .textSelection(.enabled)
                            }
                        }
                    } header: {
                        Text("Models")
                    }
                }
                if let errors = account.errors, !errors.isEmpty {
                    ErrorsSection(errors: errors)
                }
                Section("Details") {
                    LabeledContent("Name", value: account.name)
                    if account.id != account.name { LabeledContent("ID", value: account.id) }
                    if let priority = account.priority { LabeledContent("Priority", value: "\(priority)") }
                    if let note = account.note, !note.isEmpty { LabeledContent("Note", value: note) }
                    if let created = account.created {
                        LabeledContent("Added", value: created.formatted(date: .abbreviated, time: .shortened))
                    }
                    if let refreshed = account.refreshed {
                        LabeledContent("Signed in again", value: refreshed.formatted(.relative(presentation: .named)))
                    }
                }
            }
            .navigationTitle(account.title)
            .navigationBarTitleDisplayMode(.inline)
            .refreshable { await model.load() }
        } else {
            ContentUnavailableView("Account gone", systemImage: "person.crop.circle.badge.questionmark",
                                   description: Text("The gateway no longer has this account."))
        }
    }
}

struct EndpointDetail: View {
    let model: AccountsModel
    let id: String

    var body: some View {
        if let endpoint = model.endpoint(id) {
            List {
                Section {
                    HStack(spacing: 12) {
                        Image(systemName: "server.rack")
                            .foregroundStyle(Color.kouAccent)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(endpoint.title)
                                .font(.headline)
                            Text([endpoint.kindName, endpoint.host].compactMap { $0 }.joined(separator: " · "))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 4)
                        AccountStateBadge(state: endpoint.disabled == true ? "disabled" : endpoint.state)
                    }
                    Toggle("Serves requests", isOn: Binding(
                        get: { endpoint.disabled != true },
                        set: { on in Task { await model.setDisabled(endpoint, !on) } }
                    ))
                    .disabled(model.busy.contains("endpoint:" + endpoint.id))
                    if let base = endpoint.baseURL, !base.isEmpty {
                        LabeledContent("Base URL") {
                            Text(base)
                                .font(.footnote.monospaced())
                                .lineLimit(1)
                                .truncationMode(.middle)
                        }
                    }
                    LabeledContent("Keys", value: "\(endpoint.keys ?? 0)")
                    if let prefix = endpoint.prefix, !prefix.isEmpty { LabeledContent("Model prefix", value: prefix) }
                }
                if let uptime = endpoint.uptime {
                    UptimeSection(uptime: uptime, range: model.range)
                }
                if let spend = endpoint.spend {
                    SpendSection(spend: spend, unit: model.view?.unit)
                }
                if let models = endpoint.models, !models.isEmpty {
                    Section("Models") {
                        ForEach(Array(models.enumerated()), id: \.offset) { _, entry in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(entry.alias.flatMap { $0.isEmpty ? nil : $0 } ?? entry.name ?? "")
                                    .font(.footnote.monospaced())
                                if let alias = entry.alias, !alias.isEmpty, let name = entry.name, name != alias {
                                    Text(name)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                            }
                        }
                    }
                }
                if let errors = endpoint.errors, !errors.isEmpty {
                    ErrorsSection(errors: errors)
                }
            }
            .navigationTitle(endpoint.title)
            .navigationBarTitleDisplayMode(.inline)
            .refreshable { await model.load() }
        } else {
            ContentUnavailableView("Endpoint gone", systemImage: "server.rack",
                                   description: Text("The gateway no longer has this endpoint."))
        }
    }
}

/// UptimeSection is how an account's or an endpoint's requests went over
/// the range, slot by slot.
struct UptimeSection: View {
    let uptime: Uptime
    let range: String

    var body: some View {
        Section {
            if let slots = uptime.slots, !slots.isEmpty {
                Chart {
                    ForEach(Array(slots.enumerated()), id: \.offset) { i, slot in
                        BarMark(x: .value("Slot", i), y: .value("Requests", Int(slot.ok ?? 0)))
                            .foregroundStyle(by: .value("Result", "Served"))
                        BarMark(x: .value("Slot", i), y: .value("Requests", Int(slot.failed ?? 0)))
                            .foregroundStyle(by: .value("Result", "Failed"))
                    }
                }
                .chartForegroundStyleScale(["Served": Color.green, "Failed": Color.red])
                .chartXAxis(.hidden)
                .chartLegend(position: .bottom, alignment: .leading)
                .frame(height: 130)
                .padding(.vertical, 4)
            }
            LabeledContent("Served", value: Format.count(uptime.ok))
            LabeledContent("Failed", value: Format.count(uptime.failed))
            LabeledContent("Average latency", value: Format.latency(uptime.avgLatencyMS))
            LabeledContent("Tokens in · out", value: "\(Format.tokens(uptime.inputTokens)) · \(Format.tokens(uptime.outputTokens))")
            if let last = uptime.lastOK {
                LabeledContent("Last served", value: last.formatted(.relative(presentation: .named)))
            }
            if let last = uptime.lastFailure {
                LabeledContent("Last failure", value: last.formatted(.relative(presentation: .named)))
            }
        } header: {
            Text(range == "7d" ? "Requests · 7 days" : "Requests · 24 hours")
        }
    }
}

struct SpendSection: View {
    let spend: Spend
    let unit: MoneyUnit?

    var body: some View {
        Section {
            LabeledContent("Requests", value: Format.count(spend.requests))
            LabeledContent("Tokens", value: Format.tokens(spend.tokens?.total))
            if let tokens = spend.tokens {
                LabeledContent("Input", value: Format.tokens(tokens.input))
                if (tokens.cacheRead ?? 0) > 0 { LabeledContent("Cache read", value: Format.tokens(tokens.cacheRead)) }
                if (tokens.cacheWrite ?? 0) > 0 { LabeledContent("Cache write", value: Format.tokens(tokens.cacheWrite)) }
                LabeledContent("Output", value: Format.tokens(tokens.output))
                if (tokens.reasoning ?? 0) > 0 { LabeledContent("of it reasoning", value: Format.tokens(tokens.reasoning)) }
            }
            LabeledContent("At API prices", value: Format.money(spend.cost, unit: unit))
        } header: {
            Text("Used")
        } footer: {
            if let unpriced = spend.unpricedTokens, unpriced > 0 {
                Text("\(Format.tokens(unpriced)) tokens of models with no known price are not in the cost.")
            }
        }
    }
}

struct ErrorsSection: View {
    let errors: [ErrorSample]

    var body: some View {
        Section("Recent errors") {
            ForEach(Array(errors.prefix(20).enumerated()), id: \.offset) { _, error in
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 8) {
                        if let status = error.status, status > 0 {
                            Text("\(status)")
                                .font(.caption.weight(.bold).monospacedDigit())
                                .foregroundStyle(.red)
                        }
                        Text(error.model ?? "")
                            .font(.caption.monospaced())
                            .lineLimit(1)
                        Spacer(minLength: 0)
                        if let at = error.at {
                            Text(at.formatted(.relative(presentation: .named)))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    if let message = error.message, !message.isEmpty {
                        Text(message)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .lineLimit(4)
                    }
                }
            }
        }
    }
}

// MARK: Usage

struct UsageList: View {
    @Bindable var usage: UsageModel
    /// The unit of money, from the accounts' view, until the report says it.
    let unit: MoneyUnit?
    @State private var metric = Metric.cost

    enum Metric: String, CaseIterable, Identifiable {
        case cost = "Cost", tokens = "Tokens", requests = "Requests"

        var id: String { rawValue }
    }

    var body: some View {
        List {
            Section {
                Picker("Range", selection: $usage.range) {
                    Text("24h").tag("24h")
                    Text("7d").tag("7d")
                    Text("30d").tag("30d")
                    Text("90d").tag("90d")
                }
                .pickerStyle(.segmented)
            }
            .listRowBackground(Color.clear)
            .listRowInsets(EdgeInsets())

            if usage.filtered {
                Section("Filtered by") {
                    if let id = usage.model {
                        filterRow("Model", usage.report?.modelName(id) ?? id) { usage.model = nil }
                    }
                    if let id = usage.account {
                        filterRow("Account", usage.report?.accountName(id) ?? id) { usage.account = nil }
                    }
                    if let id = usage.provider {
                        filterRow("Provider", providerName(id)) { usage.provider = nil }
                    }
                }
            }

            if let report = usage.report {
                let unit = report.unit ?? unit
                totals(report, unit: unit)
                Section {
                    Picker("Show", selection: $metric) {
                        ForEach(Metric.allCases) { Text($0.rawValue).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    UsageChart(report: report, metric: metric, unit: unit)
                        .frame(height: 220)
                        .padding(.vertical, 4)
                } header: {
                    Text("Over time")
                }
                if let models = report.facets?.models, !models.isEmpty {
                    FacetSection(title: "By model", facets: models, unit: unit, metric: metric, selected: usage.model,
                                 name: { report.modelName($0) }) { id in usage.model = usage.model == id ? nil : id }
                }
                if let accounts = report.facets?.accounts, !accounts.isEmpty {
                    FacetSection(title: "By account", facets: accounts, unit: unit, metric: metric, selected: usage.account,
                                 name: { report.accountName($0) }) { id in usage.account = usage.account == id ? nil : id }
                }
                if let providers = report.facets?.providers, providers.count > 1 || usage.provider != nil {
                    FacetSection(title: "By provider", facets: providers, unit: unit, metric: metric, selected: usage.provider,
                                 name: providerName) { id in usage.provider = usage.provider == id ? nil : id }
                }
                notes(report)
            }
        }
        .overlay {
            if usage.report == nil {
                if let error = usage.error {
                    ContentUnavailableView("Usage unavailable", systemImage: "chart.bar.xaxis", description: Text(error))
                } else {
                    ProgressView()
                }
            }
        }
        .task(id: usage.key) { await usage.load() }
        .refreshable { await usage.load() }
    }

    private func providerName(_ id: String) -> String {
        if let name = usage.report?.accounts?.first(where: { $0.provider == id })?.providerName, !name.isEmpty { return name }
        return id
    }

    private func filterRow(_ title: String, _ value: String, clear: @escaping () -> Void) -> some View {
        HStack {
            LabeledContent(title, value: value)
            Button("Clear", systemImage: "xmark.circle.fill", action: clear)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private func totals(_ report: UsageReport, unit: MoneyUnit?) -> some View {
        let totals = report.totals
        let previous = report.previous
        let cost = totals?.cost?.total ?? 0
        let requests = Double(totals?.requests ?? 0)
        let tokens = Double(totals?.tokens?.total ?? 0)
        Section {
            Grid(horizontalSpacing: 10, verticalSpacing: 10) {
                GridRow {
                    UsageTile(title: "At API prices", value: Format.money(cost, unit: unit),
                              change: previous.flatMap { Format.change(cost, from: $0.cost?.total ?? 0) })
                    UsageTile(title: "Requests", value: Format.count(totals?.requests),
                              detail: (totals?.failed ?? 0) > 0 ? "\(Format.count(totals?.failed)) failed" : nil,
                              change: previous.flatMap { Format.change(requests, from: Double($0.requests ?? 0)) })
                }
                GridRow {
                    UsageTile(title: "Tokens", value: Format.tokens(totals?.tokens?.total),
                              change: previous.flatMap { Format.change(tokens, from: Double($0.tokens?.total ?? 0)) })
                    UsageTile(title: "Saved by the cache", value: Format.money(totals?.cacheSaved, unit: unit),
                              detail: "Latency \(Format.latency(totals?.avgLatencyMS))")
                }
            }
            .listRowInsets(EdgeInsets(top: 10, leading: 10, bottom: 10, trailing: 10))
        } header: {
            Text("Totals")
        } footer: {
            if report.previous != nil {
                Text("Changes are against the range before.")
            }
        }
    }

    @ViewBuilder
    private func notes(_ report: UsageReport) -> some View {
        let unpriced = report.totals?.unpricedTokens ?? 0
        let pricesError = report.pricesError ?? ""
        if unpriced > 0 || !pricesError.isEmpty || report.since != nil {
            Section {
                if let since = report.since {
                    Text("Counted since \(since.formatted(date: .abbreviated, time: .shortened)).")
                }
                if unpriced > 0 {
                    Text("\(Format.tokens(unpriced)) tokens of models with no known price are not in the cost.")
                }
                if !pricesError.isEmpty {
                    Label("Prices: \(pricesError)", systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                }
            }
            .font(.footnote)
            .foregroundStyle(.secondary)
        }
    }
}

struct UsageTile: View {
    let title: String
    let value: String
    var detail: String? = nil
    var change: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            Text(value)
                .font(.title3.weight(.semibold).monospacedDigit())
                .lineLimit(1)
                .minimumScaleFactor(0.6)
            HStack(spacing: 6) {
                if let detail { Text(detail) }
                if let change { Text(change) }
            }
            .font(.caption2.monospacedDigit())
            .foregroundStyle(.secondary)
            .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(.fill.quaternary, in: .rect(cornerRadius: 14))
    }
}

/// UsageChart stacks, slot by slot, what the models that used most used;
/// the rest is Other.
struct UsageChart: View {
    let report: UsageReport
    let metric: UsageList.Metric
    let unit: MoneyUnit?

    struct Point: Identifiable {
        let id: String
        let start: Date
        let end: Date
        let series: String
        let low: Double
        let high: Double
    }

    private static let palette: [Color] = [.kouAccent, .blue, .green, .purple, .teal, .gray]

    var body: some View {
        let (points, order) = series()
        if points.isEmpty {
            Text("No requests in this range.")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            Chart(points) { point in
                RectangleMark(xStart: .value("From", point.start), xEnd: .value("To", point.end),
                              yStart: .value(metric.rawValue, point.low), yEnd: .value(metric.rawValue, point.high))
                    .foregroundStyle(by: .value("Model", point.series))
            }
            .chartForegroundStyleScale(domain: order, range: Array(Self.palette.prefix(order.count)))
            .chartLegend(position: .bottom, alignment: .leading)
            .chartYAxis {
                AxisMarks { value in
                    AxisGridLine()
                    AxisValueLabel {
                        if let number = value.as(Double.self) {
                            Text(label(number))
                        }
                    }
                }
            }
        }
    }

    private func label(_ value: Double) -> String {
        switch metric {
        case .cost: Format.money(value, unit: unit)
        case .tokens: Format.tokens(Int64(value))
        case .requests: Format.count(Int64(value))
        }
    }

    private func value(_ row: UsageRow) -> Double {
        switch metric {
        case .cost: row.cost?.total ?? 0
        case .tokens: Double(row.tokens?.total ?? 0)
        case .requests: Double(row.requests ?? 0)
        }
    }

    /// The stacks of each slot, and the order of the models in them.
    private func series() -> ([Point], [String]) {
        let rows = report.rows ?? []
        let starts = report.starts ?? []
        let seconds = Double(report.slotSeconds ?? 3600)
        var byModel: [String: Double] = [:]
        for row in rows { byModel[row.model ?? "", default: 0] += value(row) }
        let top = byModel.filter { $0.value > 0 }.sorted { $0.value > $1.value }.prefix(5).map(\.key)
        var sums: [Int: [String: Double]] = [:]
        var seen: [String] = []
        for row in rows {
            let amount = value(row)
            guard amount > 0 else { continue }
            let id = row.model ?? ""
            let name = top.contains(id) ? report.modelName(id) : "Other"
            sums[row.slot ?? 0, default: [:]][name, default: 0] += amount
            if !seen.contains(name) { seen.append(name) }
        }
        var order: [String] = []
        for id in top {
            let name = report.modelName(id)
            if seen.contains(name), !order.contains(name) { order.append(name) }
        }
        if seen.contains("Other") { order.append("Other") }
        order = Array(order.prefix(Self.palette.count))
        var points: [Point] = []
        for (slot, values) in sums.sorted(by: { $0.key < $1.key }) {
            guard starts.indices.contains(slot) else { continue }
            let start = starts[slot]
            let end = start.addingTimeInterval(seconds * 0.82)
            var low = 0.0
            for name in order {
                guard let amount = values[name], amount > 0 else { continue }
                points.append(Point(id: "\(slot):\(name)", start: start, end: end, series: name, low: low, high: low + amount))
                low += amount
            }
        }
        return (points, order)
    }
}

/// FacetSection is what each model, account or provider used, the most
/// first; choosing one filters the report by it.
struct FacetSection: View {
    let title: String
    let facets: [UsageFacet]
    let unit: MoneyUnit?
    let metric: UsageList.Metric
    let selected: String?
    var name: (String) -> String
    var pick: (String) -> Void

    var body: some View {
        let sorted = facets.sorted { measure($0) > measure($1) }
        let most = sorted.first.map(measure) ?? 0
        Section(title) {
            ForEach(sorted) { facet in
                let share = most > 0 ? measure(facet) / most : 0
                Button {
                    pick(facet.id)
                } label: {
                    VStack(alignment: .leading, spacing: 5) {
                        // Color.primary, not .primary: in a button's label
                        // .primary is the button's tint.
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Text(name(facet.id))
                                .foregroundStyle(Color.primary)
                                .lineLimit(1)
                            if selected == facet.id {
                                Image(systemName: "line.3.horizontal.decrease.circle.fill")
                                    .foregroundStyle(Color.kouAccent)
                            }
                            Spacer(minLength: 6)
                            Text(Format.money(facet.cost, unit: unit))
                                .font(.subheadline.monospacedDigit())
                                .foregroundStyle(Color.primary)
                        }
                        GeometryReader { geometry in
                            Capsule()
                                .fill(Color.kouAccent.opacity(0.55))
                                .frame(width: max(3, geometry.size.width * share))
                        }
                        .frame(height: 4)
                        Text("\(Format.count(facet.requests ?? 0)) request\(facet.requests == 1 ? "" : "s") · \(Format.tokens(facet.tokens ?? 0)) tokens")
                            .font(.caption)
                            .foregroundStyle(Color.secondary)
                    }
                    .contentShape(.rect)
                }
            }
        }
    }

    private func measure(_ facet: UsageFacet) -> Double {
        switch metric {
        case .cost: facet.cost ?? 0
        case .tokens: Double(facet.tokens ?? 0)
        case .requests: Double(facet.requests ?? 0)
        }
    }
}
