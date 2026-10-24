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
        let range = self.range
        do {
            let view: AccountsView = try await client.get("/api/accounts", query: [URLQueryItem(name: "range", value: range)])
            // The answer for a range no longer chosen is not shown: the
            // poll's may land after the one the new range asked for.
            guard range == self.range else { return }
            self.view = view
            error = nil
        } catch is CancellationError {
        } catch {
            guard range == self.range else { return }
            self.error = error.localizedDescription
        }
    }

    func poll() async {
        while !Task.isCancelled {
            await load()
            await AppFront.poll(.seconds(20))
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
                if !accounts.isEmpty {
                    Section {
                        ForEach(accounts) { account in
                            NavigationLink(value: AccountRoute.account(account.name)) {
                                AccountRow(account: account, quota: model.quota(account), unit: view.unit)
                            }
                        }
                    } header: {
                        ListHead("Accounts", count: accounts.count) {
                            SegmentSwitch(selection: $model.range, options: .activityRanges)
                        }
                    }
                    .kouRow()
                }
                if !endpoints.isEmpty {
                    Section {
                        ForEach(endpoints) { endpoint in
                            NavigationLink(value: AccountRoute.endpoint(endpoint.id)) {
                                EndpointRow(endpoint: endpoint, unit: view.unit)
                            }
                        }
                    } header: {
                        ListHead("Endpoints", count: endpoints.count) {
                            // The range is the accounts' head's, while
                            // there are accounts to head.
                            if accounts.isEmpty {
                                SegmentSwitch(selection: $model.range, options: .activityRanges)
                            }
                        }
                    }
                    .kouRow()
                }
                ConnectionSection(connection: view.connection)
            }
        }
        .groupedBackground()
        .listSectionSpacing(22)
        .labeledContentStyle(.kou)
        .overlay {
            if model.view == nil {
                if let error = model.error {
                    KouEmpty(symbol: "person.2.slash", title: "Accounts unavailable", message: error, tone: .kouErr)
                } else {
                    LoadingSpinner()
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
        if let gateway, !gateway.isOff {
            Section {
                hero(gateway)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 2, leading: 0, bottom: 2, trailing: 0))
            }
        } else {
            Section {
                HStack(alignment: .top, spacing: 12) {
                    RowIcon("powerplug", tone: .kouFg3)
                    VStack(alignment: .leading, spacing: 6) {
                        Text("The accounts gateway is off")
                            .font(.headline)
                            .foregroundStyle(Color.kouFg)
                        Text(offText)
                            .font(.footnote)
                            .foregroundStyle(Color.kouFg3)
                    }
                }
                .padding(.vertical, 6)
            } header: {
                Eyebrow("Gateway")
            }
            .kouRow()
        }
    }

    /// The gateway at a glance, as the cockpit's head of the view: its
    /// state, where it listens, and its readings — the accounts by state
    /// over what the requests did.
    private func hero(_ gateway: GatewayView) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 7) {
                    LiveDot(color: AccountStateBadge.tone(of: gateway.state), size: 6,
                            live: gateway.state == "running" || AccountStateBadge.passing(gateway.state))
                    Eyebrow("Accounts gateway", color: .kouFg2)
                    if let version = gateway.version, !version.isEmpty {
                        Text(verbatim: version)
                            .font(.kouMeta)
                            .foregroundStyle(Color.kouFg4)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 8)
                    AccountStateBadge(state: gateway.state)
                }
                ForEach(meta(gateway), id: \.self) { line in
                    Text(verbatim: line)
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg3)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            .padding(.horizontal, 6)
            if let summary = gateway.summary {
                readings(summary)
            }
            if let error = gateway.error, !error.isEmpty {
                ErrorNote(text: error)
            }
        }
        .padding(.vertical, 2)
    }

    /// Where the gateway listens, how long it has, and what runs use.
    private func meta(_ gateway: GatewayView) -> [String] {
        var lines: [String] = []
        if let address = gateway.url ?? gateway.address, !address.isEmpty {
            lines.append(address)
        }
        var facts: [String] = []
        if let since = gateway.since {
            facts.append("up " + since.kouAge)
        }
        if let model = gateway.use?.model, !model.isEmpty {
            facts.append("runs use " + model)
        }
        if !facts.isEmpty {
            lines.append(facts.joined(separator: " · "))
        }
        return lines
    }

    private func readings(_ summary: GatewaySummary) -> some View {
        let served = summary.ok ?? 0, failed = summary.failed ?? 0
        return VStack(spacing: 0) {
            ReadingStrip(framed: false) {
                Tally(count: summary.ready, title: "Ready", color: .kouOK)
                Tally(count: summary.cooling, title: "Cooling", color: .kouWarn)
                Tally(count: summary.failing, title: "Failing", color: .kouErr)
                Tally(count: summary.disabled, title: "Off", color: .kouFg2)
            }
            if served + failed > 0 || summary.cost != nil {
                Hairline()
                ReadingStrip(framed: false) {
                    Reading(label: "Served") {
                        Text(verbatim: Format.count(served))
                            .foregroundStyle(served > 0 ? Color.kouFg : Color.kouFg4)
                    }
                    Reading(label: "Failed") {
                        Text(verbatim: Format.count(failed))
                            .foregroundStyle(failed > 0 ? Color.kouErr : Color.kouFg4)
                    }
                    if summary.cost != nil {
                        Reading(label: "At API prices") {
                            Text(verbatim: Format.money(summary.cost, unit: unit))
                                .foregroundStyle(Color.kouFg)
                        }
                    }
                }
            }
        }
        .kouPanel(radius: 12, fill: .kouCard)
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
        let n = count ?? 0
        Reading(label: title) {
            Text(verbatim: "\(n)")
                .font(.system(.title3, design: .monospaced, weight: .bold))
                .foregroundStyle(n > 0 ? color : Color.kouFg4)
                .contentTransition(.numericText())
                .animation(.snappy, value: n)
        }
    }
}

/// AccountStateBadge is the state of the gateway, an account or an endpoint,
/// as the cockpit writes it: a square and the word, in the state's colour.
struct AccountStateBadge: View {
    let state: String?

    var body: some View {
        StateMark(word: title, tone: Self.tone(of: state), blinking: Self.passing(state))
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(title)
    }

    /// Served green, waiting amber, asking the provider in the accent,
    /// broken red; the rest quiet.
    static func tone(of state: String?) -> Color {
        switch state ?? "" {
        case "ready", "running": .kouOK
        case "cooling", "starting": .kouWarn
        case "refreshing": .kouAccent
        case "error", "failed": .kouErr
        default: .kouFg3
        }
    }

    /// Whether the state passes by itself: its square blinks.
    static func passing(_ state: String?) -> Bool {
        ["cooling", "starting", "refreshing"].contains(state ?? "")
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
}

/// ProviderMark is a provider's letter on a tile outlined in its colour, as
/// the cockpit tags them: Claude's clay, Codex's violet, Gemini's teal.
struct ProviderMark: View {
    let provider: String?
    /// A symbol in place of the letter: an endpoint's server.
    var symbol: String?
    var size: CGFloat = 32

    var body: some View {
        let tone = Self.color(provider)
        let shape = RoundedRectangle(cornerRadius: .corner(size / 4), style: .continuous)
        Group {
            if let symbol {
                Image(systemName: symbol)
                    .font(.system(size: size * 0.4, weight: .semibold))
            } else {
                Text(verbatim: letter)
                    .font(.system(size: size * 0.47, weight: .heavy, design: .monospaced))
            }
        }
        .foregroundStyle(tone)
        .frame(width: size, height: size)
        .background(tone.opacity(0.12), in: shape)
        .overlay(shape.strokeBorder(tone.opacity(0.45), lineWidth: 1))
        .accessibilityHidden(true)
    }

    private var letter: String {
        (provider ?? "").first.map { String($0).uppercased() } ?? "?"
    }

    /// A provider's colour — or an endpoint kind's — as the cockpit's tags
    /// have it, darker on the light grounds.
    static func color(_ provider: String?) -> Color {
        let key = (provider ?? "").lowercased()
        func tone(_ light: String, _ dark: String) -> Color {
            Color(uiColor: Look.dynamic(light: light, dark: dark))
        }
        if key.contains("claude") || key.contains("anthropic") { return tone("#b4532b", "#d9774f") }
        if key.contains("compatible") { return tone("#7a5fd6", "#a38cf4") }
        if key.contains("codex") || key.contains("openai") { return tone("#5b55d6", "#8f8aff") }
        if key.contains("gemini") || key.contains("google") || key.contains("antigravity") || key.contains("vertex") {
            return tone("#23877b", "#3fb6a8")
        }
        if key.contains("kimi") || key.contains("moonshot") { return tone("#2f6fd6", "#4f95ff") }
        if key.contains("devin") || key.contains("meta") { return tone("#4a63d6", "#6f8cff") }
        if key.contains("qwen") || key.contains("iflow") { return tone("#8549b5", "#b07fd8") }
        if key.contains("xai") || key.contains("grok") { return .kouFg }
        return .kouFg2
    }
}

struct AccountRow: View {
    let account: Account
    let quota: Quota?
    let unit: MoneyUnit?

    var body: some View {
        VStack(alignment: .leading, spacing: 11) {
            HStack(alignment: .center, spacing: 11) {
                ProviderMark(provider: account.provider)
                VStack(alignment: .leading, spacing: 3) {
                    Text(account.title)
                        .font(.body.weight(.semibold))
                        .foregroundStyle(Color.kouFg)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    let meta = [account.providerName, account.plan].compactMap { $0 }.filter { !$0.isEmpty }
                    if !meta.isEmpty {
                        Text(verbatim: meta.joined(separator: " · "))
                            .font(.kouMeta)
                            .foregroundStyle(Color.kouFg3)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 6)
                AccountStateBadge(state: account.state)
            }
            if let windows = quota?.windows, !windows.isEmpty {
                VStack(spacing: 8) {
                    ForEach(Array(windows.prefix(3).enumerated()), id: \.offset) { _, window in
                        QuotaBar(window: window, compact: true)
                    }
                }
            }
            if let slots = account.uptime?.slots, !slots.isEmpty {
                UptimeStrip(slots: slots)
                    .frame(height: 14)
            }
            HStack(spacing: 12) {
                if let spend = account.spend {
                    // The gateway leaves out what is zero.
                    Fact(Format.count(spend.requests ?? 0), "req")
                    Fact(Format.tokens(spend.tokens?.total), "tok")
                    Fact(Format.money(spend.cost, unit: unit))
                }
                if let cooldowns = account.cooldowns, !cooldowns.isEmpty {
                    Label("\(cooldowns.count) cooling", systemImage: "snowflake")
                        .foregroundStyle(Color.kouWarn)
                }
            }
            .font(.kouMeta)
            if let message = account.message, !message.isEmpty {
                Text(message)
                    .font(.caption)
                    .foregroundStyle(account.state == "error" ? Color.kouErr : Color.kouFg3)
                    .lineLimit(2)
            }
        }
        .padding(.vertical, 6)
        .opacity(account.state == "disabled" ? 0.55 : 1)
    }
}

struct EndpointRow: View {
    let endpoint: Endpoint
    let unit: MoneyUnit?

    var body: some View {
        VStack(alignment: .leading, spacing: 11) {
            HStack(spacing: 11) {
                ProviderMark(provider: endpoint.kind, symbol: "server.rack")
                VStack(alignment: .leading, spacing: 3) {
                    Text(endpoint.title)
                        .font(.body.weight(.semibold))
                        .foregroundStyle(Color.kouFg)
                        .lineLimit(1)
                    Text(verbatim: [endpoint.kindName, endpoint.host].compactMap { $0 }.joined(separator: " · "))
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg3)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer(minLength: 6)
                AccountStateBadge(state: endpoint.disabled == true ? "disabled" : endpoint.state)
            }
            if let slots = endpoint.uptime?.slots, !slots.isEmpty {
                UptimeStrip(slots: slots)
                    .frame(height: 14)
            }
            HStack(spacing: 12) {
                Fact("\(endpoint.keys ?? 0)", endpoint.keys == 1 ? "key" : "keys")
                Fact("\(endpoint.models?.count ?? 0)", endpoint.models?.count == 1 ? "model" : "models")
                if let spend = endpoint.spend {
                    Fact(Format.count(spend.requests ?? 0), "req")
                    Fact(Format.money(spend.cost, unit: unit))
                }
            }
            .font(.kouMeta)
        }
        .padding(.vertical, 6)
        .opacity(endpoint.disabled == true ? 0.55 : 1)
    }
}

/// Fact is a number and what it counts, as the cockpit's mono lines have
/// them: the number in the second ink, what it counts in the fourth.
struct Fact: View {
    let value: String
    let unit: String?

    init(_ value: String, _ unit: String? = nil) {
        self.value = value
        self.unit = unit
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            Text(verbatim: value)
                .foregroundStyle(Color.kouFg2)
            if let unit {
                Text(verbatim: unit)
                    .foregroundStyle(Color.kouFg4)
            }
        }
        .lineLimit(1)
        .accessibilityElement(children: .combine)
    }
}

/// QuotaBar is one window of an account's limits: how much of it is used,
/// and when it starts anew.
struct QuotaBar: View {
    let window: QuotaWindow
    var compact = false

    private enum Level { case ok, warn, bad }

    var body: some View {
        let used = min(max(window.used ?? 0, 0), 100)
        VStack(alignment: .leading, spacing: compact ? 5 : 7) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(window.label)
                    .font(compact ? .kouMeta : .system(.subheadline, design: .monospaced, weight: .medium))
                    .foregroundStyle(Color.kouFg2)
                    .lineLimit(1)
                Spacer(minLength: 4)
                Text(verbatim: "\(Int(used.rounded()))%")
                    .font(.system(compact ? .caption : .subheadline, design: .monospaced, weight: .bold))
                    .foregroundStyle(level == .ok ? Color.kouFg : tint)
            }
            Meter(value: used / 100, tint: tint)
                .frame(height: compact ? 4 : 6)
            if !compact {
                HStack {
                    if let amount = window.amount, !amount.isEmpty { Text(amount) }
                    Spacer(minLength: 4)
                    if let resets = window.resetsAt {
                        Text("Resets \(resets.formatted(.relative(presentation: .named)))")
                    }
                }
                .font(.kouMeta)
                .foregroundStyle(Color.kouFg3)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private var level: Level {
        let used = window.used ?? 0
        if window.reached == true || used >= 90 { return .bad }
        return used >= 70 ? .warn : .ok
    }

    private var tint: Color {
        switch level {
        case .ok: .kouOK
        case .warn: .kouWarn
        case .bad: .kouErr
        }
    }
}

/// UptimeStrip is how the requests of each slot went, as the cockpit's
/// strip: green all served, red all failed, amber some of each, the chip's
/// grey none.
struct UptimeStrip: View {
    let slots: [Uptime.Slot]

    var body: some View {
        Canvas { context, size in
            let count = slots.count
            guard count > 0 else { return }
            let gap: CGFloat = count > 100 ? 0.5 : count > 60 ? 1 : 1.5
            let width = max((size.width - gap * CGFloat(count - 1)) / CGFloat(count), 0.5)
            for (i, slot) in slots.enumerated() {
                let ok = slot.ok ?? 0, failed = slot.failed ?? 0
                let color: Color = ok + failed == 0 ? .kouChip
                    : failed == 0 ? .kouOK.opacity(0.8)
                    : ok == 0 ? .kouErr : .kouWarn
                let rect = CGRect(x: CGFloat(i) * (width + gap), y: 0, width: width, height: size.height)
                context.fill(Path(roundedRect: rect, cornerRadius: min(1.5, width / 2)), with: .color(color))
            }
        }
        .accessibilityElement()
        .accessibilityLabel("Requests over time")
    }
}

/// ListHead heads a part of a grouped list: its name in capitals, how many
/// it holds, and what acts on all of it at the end.
struct ListHead<Trailing: View>: View {
    let title: String
    var count: Int?
    @ViewBuilder var trailing: Trailing

    init(_ title: String, count: Int? = nil, @ViewBuilder trailing: () -> Trailing) {
        self.title = title
        self.count = count
        self.trailing = trailing()
    }

    var body: some View {
        HStack(alignment: .center, spacing: 6) {
            Eyebrow(title)
            if let count {
                Text(verbatim: "\(count)")
                    .font(.kouEyebrow)
                    .foregroundStyle(Color.kouFg4)
            }
            Spacer(minLength: 8)
            trailing
        }
    }
}

extension ListHead where Trailing == EmptyView {
    init(_ title: String, count: Int? = nil) {
        self.init(title, count: count) { EmptyView() }
    }
}

/// SwitchOption is one choice of a SegmentSwitch: its value, its capitals,
/// and its name for VoiceOver.
struct SwitchOption<Value: Hashable>: Identifiable {
    let value: Value
    let short: String
    let name: String
    var id: Value { value }
}

extension Array where Element == SwitchOption<String> {
    /// The ranges the accounts' activity covers.
    static var activityRanges: Self {
        [SwitchOption(value: "24h", short: "24H", name: "Last 24 hours"),
         SwitchOption(value: "7d", short: "7D", name: "Last 7 days")]
    }

    /// The ranges a usage report covers.
    static var usageRanges: Self {
        [SwitchOption(value: "24h", short: "24H", name: "Last 24 hours"),
         SwitchOption(value: "7d", short: "7D", name: "Last 7 days"),
         SwitchOption(value: "30d", short: "30D", name: "Last 30 days"),
         SwitchOption(value: "90d", short: "90D", name: "Last 90 days")]
    }
}

/// SegmentSwitch is the cockpit's range control: the choices in mono
/// capitals in a hairline box, the one chosen in the accent.
struct SegmentSwitch<Value: Hashable>: View {
    @Binding var selection: Value
    let options: [SwitchOption<Value>]

    var body: some View {
        let box = RoundedRectangle(cornerRadius: .corner(7), style: .continuous)
        let pill = RoundedRectangle(cornerRadius: .corner(5), style: .continuous)
        HStack(spacing: 2) {
            ForEach(options) { option in
                let on = option.value == selection
                Button {
                    selection = option.value
                } label: {
                    Text(verbatim: option.short)
                        .font(.kouTag)
                        .tracking(1)
                        .lineLimit(1)
                        .foregroundStyle(on ? Color.kouAccent : Color.kouFg3)
                        .padding(.horizontal, 7)
                        .frame(minWidth: 36, minHeight: 24)
                        .background(on ? Color.kouAccentSoft : Color.clear, in: pill)
                        .contentShape(pill)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(option.name)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(2)
        .overlay(box.strokeBorder(Color.kouLine2, lineWidth: 1))
        // As wide as its words: a head's spacer is to give way, not it.
        .fixedSize()
        .animation(.snappy(duration: 0.2), value: selection)
        .sensoryFeedback(.selection, trigger: selection)
    }
}

/// ErrorNote is what went wrong, on a panel tinted the error's red.
struct ErrorNote: View {
    let text: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.caption)
            Text(verbatim: text)
                .font(.footnote)
                .fixedSize(horizontal: false, vertical: true)
        }
        .foregroundStyle(Color.kouErr)
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .kouPanel(radius: 10, fill: Color.kouErr.opacity(0.08), stroke: Color.kouErr.opacity(0.35))
    }
}

/// FlowRow sets its views in a row, going on to the next line when the row
/// is full: the filters of a report, a chart's legend.
struct FlowRow: Layout {
    var spacing: CGFloat = 8
    var lineSpacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let lines = arrange(subviews, in: proposal.width ?? .infinity)
        let height = lines.last.map { $0.y + $0.height } ?? 0
        let widest = lines.map(\.width).max() ?? 0
        return CGSize(width: proposal.width ?? widest, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for line in arrange(subviews, in: bounds.width) {
            var x = bounds.minX
            for (index, size) in line.items {
                subviews[index].place(at: CGPoint(x: x, y: bounds.minY + line.y), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
        }
    }

    private struct Line {
        var items: [(Int, CGSize)] = []
        var y: CGFloat = 0
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func arrange(_ subviews: Subviews, in width: CGFloat) -> [Line] {
        var lines = [Line()]
        for index in subviews.indices {
            var size = subviews[index].sizeThatFits(.unspecified)
            size.width = min(size.width, width)
            var line = lines[lines.count - 1]
            if !line.items.isEmpty && line.width + spacing + size.width > width {
                lines.append(Line(y: line.y + line.height + lineSpacing))
                line = lines[lines.count - 1]
            }
            line.width += (line.items.isEmpty ? 0 : spacing) + size.width
            line.height = max(line.height, size.height)
            line.items.append((index, size))
            lines[lines.count - 1] = line
        }
        return lines
    }
}

/// FilterChip is a filter a report is cut by, in the accent as what is
/// chosen is; a tap clears it.
struct FilterChip: View {
    let kind: String
    let value: String
    let clear: () -> Void

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: .corner(8), style: .continuous)
        Button(action: clear) {
            HStack(spacing: 7) {
                Eyebrow(kind, color: Color.kouAccent.opacity(0.85))
                Text(verbatim: value)
                    .font(.system(.footnote, design: .monospaced, weight: .medium))
                    .foregroundStyle(Color.kouFg)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .heavy))
                    .foregroundStyle(Color.kouFg3)
            }
            .padding(.horizontal, 10)
            .frame(minHeight: 32)
            .background(Color.kouAccentSoft, in: shape)
            .overlay(shape.strokeBorder(Color.kouAccent.opacity(0.45), lineWidth: 1))
            .contentShape(shape)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Clear the \(kind.lowercased()) filter, \(value)")
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
                        .foregroundStyle(Color.kouErr)
                }
            } header: {
                Eyebrow("Connection")
            } footer: {
                FormNote("How the cockpit's runs reach models.")
            }
            .kouRow()
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
                    VStack(alignment: .leading, spacing: 12) {
                        DetailHead(title: account.title,
                                   meta: [account.providerName, account.plan, account.kind],
                                   state: account.state) {
                            ProviderMark(provider: account.provider, size: 48)
                        }
                        if let message = account.message, !message.isEmpty {
                            if account.state == "error" {
                                ErrorNote(text: message)
                            } else {
                                Text(message)
                                    .font(.footnote)
                                    .foregroundStyle(Color.kouFg3)
                            }
                        }
                    }
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 4, trailing: 4))
                }

                Section {
                    Toggle("Serves requests", isOn: Binding(
                        get: { account.state != "disabled" },
                        set: { on in Task { await model.setDisabled(account, !on) } }
                    ))
                    // A toggle is a labeled content too: the mono value's
                    // baseline would set the switch below its label.
                    .labeledContentStyle(.automatic)
                    .disabled(model.busy.contains("account:" + account.name))
                    if let retry = account.retryAt, retry > Date() {
                        LabeledContent("Tries again", value: retry.formatted(.relative(presentation: .named)))
                    }
                }
                .kouRow()

                Section {
                    if let quota {
                        ForEach(Array((quota.windows ?? []).enumerated()), id: \.offset) { _, window in
                            QuotaBar(window: window)
                                .padding(.vertical, 4)
                        }
                        ForEach(quota.notes ?? [], id: \.self) { note in
                            Text(note)
                                .font(.footnote)
                                .foregroundStyle(Color.kouFg3)
                        }
                        if let error = quota.error, !error.isEmpty {
                            Text(error)
                                .font(.footnote)
                                .foregroundStyle(Color.kouErr)
                        }
                    } else {
                        Text("The gateway has not read this account's limits yet.")
                            .font(.subheadline)
                            .foregroundStyle(Color.kouFg3)
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
                    Eyebrow("Limits")
                } footer: {
                    if let quota, let at = quota.at {
                        FormNote(verbatim: "\(quota.source == "live" ? "Asked" : "Seen in a response") \(at.formatted(.relative(presentation: .named))).")
                    }
                }
                .kouRow()

                if let cooldowns = account.cooldowns, !cooldowns.isEmpty {
                    Section {
                        ForEach(Array(cooldowns.enumerated()), id: \.offset) { _, cooldown in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(cooldown.model ?? "Every model")
                                    .font(.system(.subheadline, design: .monospaced, weight: .medium))
                                    .foregroundStyle(Color.kouFg)
                                HStack(spacing: 8) {
                                    if let reason = cooldown.reason, !reason.isEmpty {
                                        Text(reason)
                                            .foregroundStyle(Color.kouWarn)
                                    }
                                    if let status = cooldown.status, status > 0 { Text(verbatim: "HTTP \(status)") }
                                    Spacer(minLength: 0)
                                    if let until = cooldown.until {
                                        Text("until \(until.formatted(date: .omitted, time: .shortened))")
                                    }
                                }
                                .font(.kouMeta)
                                .foregroundStyle(Color.kouFg3)
                            }
                            .padding(.vertical, 2)
                        }
                    } header: {
                        ListHead("Cooling down", count: cooldowns.count)
                    }
                    .kouRow()
                }

                if let uptime = account.uptime {
                    UptimeSection(uptime: uptime, range: model.range)
                }
                if let spend = account.spend {
                    SpendSection(spend: spend, unit: unit)
                }
                if let models = account.models, !models.isEmpty {
                    Section {
                        DisclosureGroup(isExpanded: $showsModels) {
                            ForEach(models, id: \.self) { id in
                                Text(id)
                                    .font(.footnote.monospaced())
                                    .foregroundStyle(Color.kouFg2)
                                    .textSelection(.enabled)
                            }
                        } label: {
                            Text("\(models.count) model\(models.count == 1 ? "" : "s")")
                                .foregroundStyle(Color.kouFg)
                        }
                        .tint(Color.kouFg3)
                    } header: {
                        ListHead("Models", count: models.count)
                    }
                    .kouRow()
                }
                if let errors = account.errors, !errors.isEmpty {
                    ErrorsSection(errors: errors)
                }
                Section {
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
                } header: {
                    Eyebrow("Details")
                }
                .kouRow()
            }
            .groupedBackground()
            .listSectionSpacing(22)
            .labeledContentStyle(.kou)
            .headTitle(account.title)
            .refreshable { await model.load() }
        } else {
            KouEmpty(symbol: "person.crop.circle.badge.questionmark", title: "Account gone",
                     message: "The gateway no longer has this account.")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color.kouGroupedGround)
        }
    }
}

/// DetailHead heads an account's screen or an endpoint's: its mark, its
/// name, what it is, and how it is.
struct DetailHead<Mark: View>: View {
    let title: String
    let meta: [String?]
    let state: String?
    @ViewBuilder var mark: Mark

    var body: some View {
        HStack(alignment: .center, spacing: 14) {
            mark
            VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: title)
                    .font(.title3.weight(.bold))
                    .foregroundStyle(Color.kouFg)
                    .lineLimit(2)
                    .truncationMode(.middle)
                let facts = meta.compactMap { $0 }.filter { !$0.isEmpty }
                if !facts.isEmpty {
                    Text(verbatim: facts.joined(separator: " · "))
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg3)
                        .lineLimit(1)
                }
                AccountStateBadge(state: state)
                    .padding(.top, 3)
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 4)
    }
}

struct EndpointDetail: View {
    let model: AccountsModel
    let id: String

    var body: some View {
        if let endpoint = model.endpoint(id) {
            List {
                Section {
                    DetailHead(title: endpoint.title, meta: [endpoint.kindName, endpoint.host],
                               state: endpoint.disabled == true ? "disabled" : endpoint.state) {
                        ProviderMark(provider: endpoint.kind, symbol: "server.rack", size: 48)
                    }
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 4, trailing: 4))
                }
                Section {
                    Toggle("Serves requests", isOn: Binding(
                        get: { endpoint.disabled != true },
                        set: { on in Task { await model.setDisabled(endpoint, !on) } }
                    ))
                    .labeledContentStyle(.automatic)
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
                .kouRow()
                if let uptime = endpoint.uptime {
                    UptimeSection(uptime: uptime, range: model.range)
                }
                if let spend = endpoint.spend {
                    SpendSection(spend: spend, unit: model.view?.unit)
                }
                if let models = endpoint.models, !models.isEmpty {
                    Section {
                        ForEach(Array(models.enumerated()), id: \.offset) { _, entry in
                            VStack(alignment: .leading, spacing: 3) {
                                Text(entry.alias.flatMap { $0.isEmpty ? nil : $0 } ?? entry.name ?? "")
                                    .font(.footnote.monospaced())
                                    .foregroundStyle(Color.kouFg)
                                if let alias = entry.alias, !alias.isEmpty, let name = entry.name, name != alias {
                                    Text(name)
                                        .font(.kouMeta)
                                        .foregroundStyle(Color.kouFg3)
                                }
                            }
                        }
                    } header: {
                        ListHead("Models", count: models.count)
                    }
                    .kouRow()
                }
                if let errors = endpoint.errors, !errors.isEmpty {
                    ErrorsSection(errors: errors)
                }
            }
            .groupedBackground()
            .listSectionSpacing(22)
            .labeledContentStyle(.kou)
            .headTitle(endpoint.title)
            .refreshable { await model.load() }
        } else {
            KouEmpty(symbol: "server.rack", title: "Endpoint gone",
                     message: "The gateway no longer has this endpoint.")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color.kouGroupedGround)
        }
    }
}

/// UptimeSection is how an account's or an endpoint's requests went over
/// the range, slot by slot: its readings over the trace.
struct UptimeSection: View {
    let uptime: Uptime
    let range: String

    var body: some View {
        Section {
            VStack(spacing: 4) {
                ReadingStrip(framed: false) {
                    Reading(label: "Served") {
                        Text(verbatim: Format.count(uptime.ok))
                            .foregroundStyle((uptime.ok ?? 0) > 0 ? Color.kouOK : Color.kouFg4)
                    }
                    Reading(label: "Failed") {
                        Text(verbatim: Format.count(uptime.failed))
                            .foregroundStyle((uptime.failed ?? 0) > 0 ? Color.kouErr : Color.kouFg4)
                    }
                    Reading(label: "Avg latency") {
                        Text(verbatim: Format.latency(uptime.avgLatencyMS))
                            .foregroundStyle(Color.kouFg)
                    }
                }
                if let slots = uptime.slots, !slots.isEmpty {
                    trace(slots)
                        .padding(.bottom, 10)
                }
            }
            LabeledContent("Tokens in · out", value: "\(Format.tokens(uptime.inputTokens)) · \(Format.tokens(uptime.outputTokens))")
            if let last = uptime.lastOK {
                LabeledContent("Last served", value: last.formatted(.relative(presentation: .named)))
            }
            if let last = uptime.lastFailure {
                LabeledContent("Last failure", value: last.formatted(.relative(presentation: .named)))
            }
        } header: {
            Eyebrow(range == "7d" ? "Requests · 7 days" : "Requests · 24 hours")
        }
        .kouRow()
    }

    /// The requests of each slot, the served under the failed.
    private func trace(_ slots: [Uptime.Slot]) -> some View {
        Chart {
            ForEach(Array(slots.enumerated()), id: \.offset) { i, slot in
                BarMark(x: .value("Slot", i), y: .value("Requests", Int(slot.ok ?? 0)))
                    .foregroundStyle(by: .value("Result", "Served"))
                BarMark(x: .value("Slot", i), y: .value("Requests", Int(slot.failed ?? 0)))
                    .foregroundStyle(by: .value("Result", "Failed"))
            }
        }
        .chartForegroundStyleScale(["Served": Color.kouOK.opacity(0.85), "Failed": Color.kouErr])
        .chartLegend(.hidden)
        .chartXAxis(.hidden)
        .chartYAxis {
            AxisMarks(position: .trailing, values: .automatic(desiredCount: 3)) { _ in
                AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5, dash: [2, 3]))
                    .foregroundStyle(Color.kouLine2)
                AxisValueLabel()
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg4)
            }
        }
        .frame(height: 110)
        .accessibilityLabel("Requests over time")
    }
}

struct SpendSection: View {
    let spend: Spend
    let unit: MoneyUnit?

    var body: some View {
        Section {
            ReadingStrip(framed: false) {
                Reading(label: "Requests") {
                    Text(verbatim: Format.count(spend.requests))
                        .foregroundStyle(Color.kouFg)
                }
                Reading(label: "Tokens") {
                    Text(verbatim: Format.tokens(spend.tokens?.total))
                        .foregroundStyle(Color.kouFg)
                }
                Reading(label: "At API prices") {
                    Text(verbatim: Format.money(spend.cost, unit: unit))
                        .foregroundStyle(Color.kouFg)
                }
            }
            if let tokens = spend.tokens {
                LabeledContent("Input", value: Format.tokens(tokens.input))
                if (tokens.cacheRead ?? 0) > 0 { LabeledContent("Cache read", value: Format.tokens(tokens.cacheRead)) }
                if (tokens.cacheWrite ?? 0) > 0 { LabeledContent("Cache write", value: Format.tokens(tokens.cacheWrite)) }
                LabeledContent("Output", value: Format.tokens(tokens.output))
                if (tokens.reasoning ?? 0) > 0 { LabeledContent("of it reasoning", value: Format.tokens(tokens.reasoning)) }
            }
        } header: {
            Eyebrow("Used")
        } footer: {
            if let unpriced = spend.unpricedTokens, unpriced > 0 {
                FormNote(verbatim: "\(Format.tokens(unpriced)) tokens of models with no known price are not in the cost.")
            }
        }
        .kouRow()
    }
}

struct ErrorsSection: View {
    let errors: [ErrorSample]

    var body: some View {
        Section {
            ForEach(Array(errors.prefix(20).enumerated()), id: \.offset) { _, error in
                VStack(alignment: .leading, spacing: 5) {
                    HStack(spacing: 8) {
                        if let status = error.status, status > 0 {
                            Tag("\(status)", tone: .kouErr)
                        }
                        Text(verbatim: error.model ?? "")
                            .font(.kouMeta)
                            .foregroundStyle(Color.kouFg2)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Spacer(minLength: 0)
                        if let at = error.at {
                            Text(verbatim: at.kouAge)
                                .font(.kouMeta)
                                .foregroundStyle(Color.kouFg4)
                        }
                    }
                    if let message = error.message, !message.isEmpty {
                        Text(message)
                            .font(.footnote)
                            .foregroundStyle(Color.kouFg3)
                            .lineLimit(4)
                    }
                }
                .padding(.vertical, 3)
            }
        } header: {
            ListHead("Recent errors", count: errors.count)
        }
        .kouRow()
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

        static var options: [SwitchOption<Metric>] {
            allCases.map { SwitchOption(value: $0, short: $0.rawValue.uppercased(), name: $0.rawValue) }
        }
    }

    var body: some View {
        List {
            Section {
                // The rows' insets, not the section's: on a section they
                // would move its head and foot off the line of the others.
                Group {
                    if usage.filtered {
                        FlowRow(spacing: 6, lineSpacing: 6) {
                            if let id = usage.model {
                                FilterChip(kind: "Model", value: usage.report?.modelName(id) ?? id) { usage.model = nil }
                            }
                            if let id = usage.account {
                                FilterChip(kind: "Account", value: usage.report?.accountName(id) ?? id) { usage.account = nil }
                            }
                            if let id = usage.provider {
                                FilterChip(kind: "Provider", value: providerName(id)) { usage.provider = nil }
                            }
                        }
                    }
                    if let report = usage.report {
                        totals(report, unit: report.unit ?? unit)
                    }
                }
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0))
                .listRowSeparator(.hidden)
            } header: {
                ListHead("Totals") {
                    SegmentSwitch(selection: $usage.range, options: .usageRanges)
                }
            } footer: {
                if usage.report?.previous != nil {
                    FormNote("Changes are against the range before.")
                }
            }

            if let report = usage.report {
                let unit = report.unit ?? unit
                Section {
                    UsageChart(report: report, metric: metric, unit: unit)
                        .frame(height: 240)
                        .padding(.vertical, 8)
                } header: {
                    ListHead("Over time") {
                        SegmentSwitch(selection: $metric, options: Metric.options)
                    }
                }
                .kouRow()
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
        .groupedBackground()
        .listSectionSpacing(22)
        .overlay {
            if usage.report == nil {
                if let error = usage.error {
                    KouEmpty(symbol: "chart.bar.xaxis", title: "Usage unavailable", message: error, tone: .kouErr)
                } else {
                    LoadingSpinner()
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

    /// The four readings of the report, on tiles of their own.
    private func totals(_ report: UsageReport, unit: MoneyUnit?) -> some View {
        let totals = report.totals
        let previous = report.previous
        let cost = totals?.cost?.total ?? 0
        let requests = Double(totals?.requests ?? 0)
        let tokens = Double(totals?.tokens?.total ?? 0)
        return Grid(horizontalSpacing: 10, verticalSpacing: 10) {
            GridRow {
                UsageTile(title: "At API prices", value: Format.money(cost, unit: unit),
                          change: previous.flatMap { Format.change(cost, from: $0.cost?.total ?? 0) })
                UsageTile(title: "Requests", value: Format.count(totals?.requests),
                          detail: (totals?.failed ?? 0) > 0 ? "\(Format.count(totals?.failed)) failed" : nil,
                          detailTone: .kouErr,
                          change: previous.flatMap { Format.change(requests, from: Double($0.requests ?? 0)) })
            }
            GridRow {
                UsageTile(title: "Tokens", value: Format.tokens(totals?.tokens?.total),
                          change: previous.flatMap { Format.change(tokens, from: Double($0.tokens?.total ?? 0)) })
                UsageTile(title: "Saved by the cache", value: Format.money(totals?.cacheSaved, unit: unit),
                          detail: "latency \(Format.latency(totals?.avgLatencyMS))")
            }
        }
    }

    @ViewBuilder
    private func notes(_ report: UsageReport) -> some View {
        let unpriced = report.totals?.unpricedTokens ?? 0
        let pricesError = report.pricesError ?? ""
        if unpriced > 0 || !pricesError.isEmpty || report.since != nil {
            Section {
                VStack(alignment: .leading, spacing: 6) {
                    if let since = report.since {
                        FormNote(verbatim: "Counted since \(since.formatted(date: .abbreviated, time: .shortened)).")
                    }
                    if unpriced > 0 {
                        FormNote(verbatim: "\(Format.tokens(unpriced)) tokens of models with no known price are not in the cost.")
                    }
                    if !pricesError.isEmpty {
                        Label("Prices: \(pricesError)", systemImage: "exclamationmark.triangle")
                            .font(.footnote)
                            .foregroundStyle(Color.kouWarn)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets(top: 0, leading: 6, bottom: 0, trailing: 6))
            }
        }
    }
}

/// UsageTile is one reading of a usage report: what it reads in capitals,
/// the number in heavy mono, and how it moved since the range before.
struct UsageTile: View {
    let title: String
    let value: String
    var detail: String? = nil
    var detailTone: Color = .kouFg3
    var change: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            Eyebrow(title, color: .kouFg3)
            Text(verbatim: value)
                .font(.system(.title2, design: .monospaced, weight: .bold))
                .foregroundStyle(Color.kouFg)
                .lineLimit(1)
                .minimumScaleFactor(0.55)
                .contentTransition(.numericText())
            HStack(spacing: 8) {
                if let change {
                    Text(verbatim: change)
                        .foregroundStyle(Color.kouFg2)
                }
                if let detail {
                    Text(verbatim: detail)
                        .foregroundStyle(detailTone)
                }
                if change == nil && detail == nil {
                    // A line all the same, so that the tiles of a row match.
                    Text(verbatim: " ")
                }
            }
            .font(.kouMeta)
            .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .kouPanel(radius: 12, fill: .kouCard)
        .accessibilityElement(children: .combine)
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

    /// The colours of the models: the accent as chosen (Look) for the one
    /// that used most, then the cockpit's series, Other the quietest.
    private static var palette: [Color] {
        [.kouAccent,
         Color(uiColor: Look.dynamic(light: "#23877b", dark: "#3fb6a8")),
         Color(uiColor: Look.dynamic(light: "#5b55d6", dark: "#8f8aff")),
         Color(uiColor: Look.dynamic(light: "#2f6fd6", dark: "#4f95ff")),
         .kouWarn,
         .kouFg3]
    }

    var body: some View {
        let (points, order) = series()
        if points.isEmpty {
            VStack(spacing: 8) {
                Image(systemName: "chart.bar.xaxis")
                    .font(.title3)
                    .foregroundStyle(Color.kouFg4)
                Text("No requests in this range.")
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg3)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            let colors = Array(Self.palette.prefix(order.count))
            Chart(points) { point in
                RectangleMark(xStart: .value("From", point.start), xEnd: .value("To", point.end),
                              yStart: .value(metric.rawValue, point.low), yEnd: .value(metric.rawValue, point.high))
                    .foregroundStyle(by: .value("Model", point.series))
            }
            .chartForegroundStyleScale(domain: order, range: colors)
            .chartLegend(position: .bottom, alignment: .leading, spacing: 14) {
                FlowRow(spacing: 12, lineSpacing: 6) {
                    ForEach(order.indices, id: \.self) { i in
                        HStack(spacing: 5) {
                            RoundedRectangle(cornerRadius: 2, style: .continuous)
                                .fill(colors[i])
                                .frame(width: 8, height: 8)
                            Text(verbatim: order[i])
                                .font(.kouMeta)
                                .foregroundStyle(Color.kouFg2)
                                .lineLimit(1)
                                .truncationMode(.middle)
                        }
                    }
                }
            }
            .chartXAxis {
                AxisMarks(values: .automatic(desiredCount: 4)) { _ in
                    AxisValueLabel()
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg4)
                }
            }
            .chartYAxis {
                AxisMarks(position: .trailing, values: .automatic(desiredCount: 4)) { value in
                    AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5, dash: [2, 3]))
                        .foregroundStyle(Color.kouLine2)
                    AxisValueLabel {
                        if let number = value.as(Double.self) {
                            Text(label(number))
                        }
                    }
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg4)
                }
            }
        }
    }

    private func label(_ value: Double) -> String {
        switch metric {
        case .cost where Format.isDollars(unit) && value >= 1 && value == value.rounded():
            // The axis' marks are round: their cents say nothing.
            "$" + value.formatted(.number.precision(.fractionLength(0)))
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
        Section {
            ForEach(sorted) { facet in
                let share = most > 0 ? measure(facet) / most : 0
                let on = selected == facet.id
                Button {
                    pick(facet.id)
                } label: {
                    VStack(alignment: .leading, spacing: 7) {
                        // Colours of the kit's, not .primary: in a button's
                        // label .primary is the button's tint.
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Text(name(facet.id))
                                .font(.subheadline.weight(on ? .semibold : .regular))
                                .foregroundStyle(on ? Color.kouAccent : Color.kouFg)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            if on {
                                Image(systemName: "line.3.horizontal.decrease")
                                    .font(.caption.weight(.bold))
                                    .foregroundStyle(Color.kouAccent)
                            }
                            Spacer(minLength: 6)
                            Text(verbatim: Format.money(facet.cost, unit: unit))
                                .font(.system(.subheadline, design: .monospaced, weight: .semibold))
                                .foregroundStyle(Color.kouFg)
                        }
                        Meter(value: share, tint: on ? .kouAccent : .kouFg3)
                            .frame(height: 4)
                        HStack(spacing: 12) {
                            Fact(Format.count(facet.requests ?? 0), facet.requests == 1 ? "request" : "requests")
                            Fact(Format.tokens(facet.tokens ?? 0), "tokens")
                        }
                        .font(.kouMeta)
                    }
                    .padding(.vertical, 3)
                    .contentShape(.rect)
                }
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        } header: {
            ListHead(title, count: facets.count)
        }
        .kouRow()
    }

    private func measure(_ facet: UsageFacet) -> Double {
        switch metric {
        case .cost: facet.cost ?? 0
        case .tokens: Double(facet.tokens ?? 0)
        case .requests: Double(facet.requests ?? 0)
        }
    }
}
