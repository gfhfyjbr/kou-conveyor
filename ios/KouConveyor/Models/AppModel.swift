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
        case sessions, canvases, relay
    }

    private(set) var relay: RelayConfig?
    private(set) var client: RelayClient?
    private(set) var link: Link = .checking
    private(set) var health: RelayHealth?
    private(set) var workspaces: [Workspace] = []
    private(set) var tunnel: TunnelStatus?
    private(set) var workspaceID: String?
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
        link = .checking
        generation += 1
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
}
