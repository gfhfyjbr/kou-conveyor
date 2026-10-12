import Foundation
import Observation

/// AppModel is the paired relay and what the app knows of the cockpit behind
/// it: whether it is reachable, its workspaces, and its tunnel.
@MainActor
@Observable
final class AppModel {
    enum Link: Equatable {
        /// Not asked yet, or being asked.
        case checking
        /// The relay answers and the cockpit is connected to it.
        case online
        /// The relay answers; the cockpit is not connected to it.
        case cockpitOffline
        case failed(String)

        var isOnline: Bool { self == .online }
    }

    enum Tab: Hashable {
        case sessions, canvases, files, accounts, relay
    }

    private(set) var relay: RelayConfig?
    private(set) var client: RelayClient?
    private(set) var link: Link = .checking
    private(set) var health: RelayHealth?
    private(set) var workspaces: [Workspace] = []
    private(set) var tunnel: TunnelStatus?
    private(set) var workspaceID: String?
    /// The cockpit's settings: the effort and its levels, its model.
    private(set) var config: CockpitConfig?
    /// What runs go with: the effort, the tools, the sandbox.
    private(set) var preferences: Preferences?
    /// A preference being saved, shown as chosen until the cockpit has it.
    private(set) var saving: PreferencesChange?
    private var settingsAt: Date?
    /// The models of each workspace, and when they were asked for.
    private var catalogs: [String: (catalog: ModelCatalog, at: Date)] = [:]
    /// Changes with the relay: screens of the one before start over.
    private(set) var generation = 0
    var tab: Tab = .sessions
    /// A pairing link opened while the app is paired with another relay,
    /// which the user is asked about.
    var incomingLink: String?
    /// What went wrong with a link opened from outside.
    var linkError: String?

    private static let workspaceKey = "workspace"

    init() {
        if let saved = RelayStore.load() {
            relay = saved
            client = RelayClient(config: saved)
        }
        workspaceID = UserDefaults.standard.string(forKey: Self.workspaceKey)
    }

    var isPaired: Bool { relay != nil }

    /// The workspace shown: the one chosen, else the cockpit's own.
    var workspace: Workspace? {
        workspaces.first { $0.id == workspaceID } ?? workspaces.first { $0.startup == true } ?? workspaces.first
    }

    func select(workspace: Workspace) {
        workspaceID = workspace.id
        UserDefaults.standard.set(workspace.id, forKey: Self.workspaceKey)
    }

    // MARK: Pairing

    /// Pairs with the relay of a link, once the relay has taken its token.
    func pair(link: String) async throws {
        try await pair(RelayConfig(link: link))
    }

    func pair(_ config: RelayConfig) async throws {
        let candidate = RelayClient(config: config)
        let health = try await candidate.health()
        try RelayStore.save(config)
        relay = config
        client = candidate
        generation += 1
        workspaces = []
        tunnel = nil
        forgetSettings()
        apply(health)
        tab = .sessions
        await refresh()
    }

    func unpair() {
        RelayStore.clear()
        relay = nil
        client = nil
        health = nil
        workspaces = []
        tunnel = nil
        forgetSettings()
        link = .checking
        generation += 1
    }

    private func forgetSettings() {
        config = nil
        preferences = nil
        saving = nil
        settingsAt = nil
        catalogs = [:]
    }

    /// A link opened from outside: the Camera app read the pairing code,
    /// or the user tapped one.
    func open(_ url: URL) {
        let text = url.absoluteString
        guard let config = try? RelayConfig(link: text) else {
            linkError = PairingError.notALink.errorDescription
            return
        }
        if relay == nil || relay == config {
            Task { await pairFromLink(text) }
        } else {
            incomingLink = text
        }
    }

    func pairFromLink(_ text: String) async {
        do {
            try await pair(link: text)
        } catch {
            linkError = error.localizedDescription
        }
    }

    // MARK: Watching the cockpit

    /// refresh asks the relay how it and the cockpit are, then the cockpit
    /// for its workspaces and its tunnel.
    func refresh() async {
        guard let client else { return }
        let generation = self.generation
        do {
            let health = try await client.health()
            guard generation == self.generation else { return }
            apply(health)
            guard health.agent == true else { return }
            async let workspaces: [Workspace] = client.get("/api/workspaces")
            async let tunnel: TunnelStatus? = try? client.get("/api/tunnel")
            let (list, status) = try await (workspaces, tunnel)
            guard generation == self.generation else { return }
            self.workspaces = list.filter { $0.missing != true }
            self.tunnel = status
            if settingsAt.map({ Date().timeIntervalSince($0) > 60 }) ?? true {
                await loadSettings()
            }
        } catch is CancellationError {
        } catch let error as RelayError where error == .cockpitOffline {
            guard generation == self.generation else { return }
            link = .cockpitOffline
        } catch {
            guard generation == self.generation else { return }
            link = .failed(error.localizedDescription)
        }
    }

    /// Something the app asked failed: whether the relay or the cockpit went
    /// away says the next refresh, now.
    func noticed(_ error: any Error) {
        guard let error = error as? RelayError, error.isConnectivity else { return }
        switch error {
        case .cockpitOffline: link = .cockpitOffline
        default: link = .failed(error.localizedDescription)
        }
    }

    /// watch refreshes while the app is in front: often while the cockpit is
    /// out of reach, so it is back as soon as it is.
    func watch() async {
        while !Task.isCancelled {
            await refresh()
            let pause: Duration = link.isOnline ? .seconds(15) : .seconds(4)
            try? await Task.sleep(for: pause)
        }
    }

    private func apply(_ health: RelayHealth) {
        self.health = health
        link = health.agent == true ? .online : .cockpitOffline
    }

    // MARK: Settings

    /// The levels of effort, as the cockpit's page has them.
    static let effortLevels = ["low", "medium", "high", "xhigh", "max"]

    /// loadSettings asks the cockpit for its settings and its preferences.
    func loadSettings() async {
        guard let client else { return }
        let generation = self.generation
        async let config: CockpitConfig? = try? client.get("/api/config")
        async let preferences: Preferences? = try? client.get("/api/preferences")
        let (settings, chosen) = await (config, preferences)
        guard generation == self.generation else { return }
        if let settings { self.config = settings }
        if let chosen { self.preferences = chosen }
        settingsAt = Date()
    }

    var effortLevels: [String] {
        if let levels = config?.thinkingLevels, !levels.isEmpty { return levels }
        return Self.effortLevels
    }

    /// The effort of the next runs: the one chosen in either cockpit, else
    /// the server's own.
    var effort: String {
        knownEffort ?? "high"
    }

    /// The effort, once the cockpit said it: what a run is sent with.
    var knownEffort: String? {
        saving?.effort ?? preferences?.effort ?? config?.thinking
    }

    var toolProfile: String? { saving?.toolProfile ?? preferences?.toolProfile }
    var sandbox: String? { saving?.sandbox ?? preferences?.sandbox }

    /// save keeps a preference in the cockpit, for its page and the terminal
    /// cockpit too; it says what went wrong.
    @discardableResult
    func save(_ change: PreferencesChange) async -> String? {
        guard let client else { return "This iPhone is not paired with a relay." }
        saving = change
        defer { saving = nil }
        do {
            let saved: Preferences = try await client.send("PUT", "/api/preferences", body: change)
            preferences = saved
            return nil
        } catch {
            noticed(error)
            return error.localizedDescription
        }
    }

    // MARK: Models

    /// The models of a workspace, as last asked for.
    func catalog(_ workspace: String) -> ModelCatalog? {
        catalogs[workspace]?.catalog
    }

    /// loadCatalog asks for the models of a workspace, unless it did a
    /// moment ago; fresh asks the providers again. It says what went wrong.
    @discardableResult
    func loadCatalog(_ workspace: String, fresh: Bool = false) async -> String? {
        guard let client else { return nil }
        if !fresh, let known = catalogs[workspace], Date().timeIntervalSince(known.at) < 120 { return nil }
        do {
            let catalog: ModelCatalog = try await client.get(API.workspace(workspace, "models"),
                                                             query: fresh ? [URLQueryItem(name: "fresh", value: "1")] : [])
            catalogs[workspace] = (catalog, Date())
            return catalog.error.flatMap { $0.isEmpty ? nil : $0 }
        } catch is CancellationError {
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    /// The model a workspace's runs take unless one is chosen.
    func defaultModel(_ workspace: String) -> String? {
        if let model = catalog(workspace)?.defaultModel, !model.isEmpty { return model }
        if let model = workspaces.first(where: { $0.id == workspace })?.connection?.model, !model.isEmpty { return model }
        return config?.model
    }

    /// How a model reads: its name, else its ID.
    func modelTitle(_ id: String?, in workspace: String) -> String {
        guard let id, !id.isEmpty else { return "Default model" }
        return catalog(workspace)?.find(id)?.title ?? id
    }
}
