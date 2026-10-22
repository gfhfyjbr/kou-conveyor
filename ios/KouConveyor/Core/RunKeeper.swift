import BackgroundTasks
import Foundation
import UIKit
import UserNotifications

/// The two background features the user can turn off, kept in UserDefaults so
/// the models and the toggles in the Relay screen read the same value. Both
/// default on.
enum BackgroundSettings {
    static let keepAliveKey = "background.keepAlive"
    static let notifyKey = "background.notifyTurnEnd"

    static var keepAlive: Bool { UserDefaults.standard.object(forKey: keepAliveKey) as? Bool ?? true }
    static var notify: Bool { UserDefaults.standard.object(forKey: notifyKey) as? Bool ?? true }
}

/// AppFront is whether the app is in front, for what goes differently while
/// it is away: polls stand still, and a stream that broke meanwhile is wanted
/// back at once when the app is in front again.
@MainActor
enum AppFront {
    private(set) static var active = true
    /// How many times the app came to the front again.
    private(set) static var arrivals = 0
    private static var arrived = ContinuousClock.now

    static func update(active: Bool) {
        guard active != self.active else { return }
        self.active = active
        guard active else { return }
        arrivals += 1
        arrived = .now
        RunKeeper.shared.cameToFront()
    }

    /// Whether the app came to the front a moment ago.
    static var justArrived: Bool { active && ContinuousClock.now - arrived < .seconds(3) }

    /// pause waits before a stream is tried again; the app coming to the
    /// front cuts it short.
    static func pause(_ wait: Duration) async {
        let since = arrivals
        let end = ContinuousClock.now + wait
        while ContinuousClock.now < end, arrivals == since, !Task.isCancelled {
            try? await Task.sleep(for: min(.milliseconds(250), end - ContinuousClock.now))
        }
    }

    /// poll waits for a poll's next round and, while the app is away, until
    /// it is back: in the background only a run's stream is kept going.
    static func poll(_ wait: Duration) async {
        try? await Task.sleep(for: wait)
        while !active, !Task.isCancelled {
            try? await Task.sleep(for: .seconds(1))
        }
    }
}

/// BackgroundTime is a stretch of plain background time (about 30 seconds
/// once the app is away), ended once, by its holder or when it runs out.
@MainActor
final class BackgroundTime {
    private var id = UIBackgroundTaskIdentifier.invalid

    init(_ name: String) {
        id = UIApplication.shared.beginBackgroundTask(withName: name) { [weak self] in
            // Called on the main thread, and it must end the task before it returns.
            MainActor.assumeIsolated { self?.end() }
        }
    }

    var active: Bool { id != .invalid }

    func end() {
        guard id != .invalid else { return }
        UIApplication.shared.endBackgroundTask(id)
        id = .invalid
    }
}

/// RunKeeper holds the app alive while a run goes on, so the run's event
/// stream need not reconnect when the app is sent to the background: a
/// `BGContinuedProcessingTask` (iOS 26), which the system keeps running after
/// the app leaves the front and shows in a Live Activity the user can cancel.
/// It is counted: the first run going on begins it, the last one to end ends
/// it. It is capped, and the system grants no time for sure: it may end it
/// sooner (load, heat, the user). The streams then reconnect once the app is
/// in front again, and the task begins anew while runs go on.
@MainActor
final class RunKeeper {
    static let shared = RunKeeper()
    private init() {}

    /// How long the app is held in the background at most.
    private static let cap: Duration = .seconds(20 * 60)
    private static let prefix = "\(Bundle.main.bundleIdentifier ?? "io.github.gfhfyjbr.kouconveyor").run"

    /// How many runs go on.
    private var holders = 0
    /// Ends the task a moment after the last run let it go.
    private var letGo: Task<Void, Never>?
    private var task: BGContinuedProcessingTask?
    /// The identifier of the task asked for last, whose start is awaited.
    private var submitted: String?
    private var ticker: Task<Void, Never>?
    /// Plain background time, for the gap until the continued task runs and
    /// for where it does not (the Simulator; the system declining it now).
    private var fallback: BackgroundTime?
    private var title = "kou-conveyor"
    private var subtitle = "The agent is working"

    /// hold marks one more run going on; balanced by release.
    func hold() {
        holders += 1
        letGo?.cancel()
        letGo = nil
        if holders == 1 { begin() }
    }

    /// release marks a run ended; the last one lets the app go, a moment
    /// later: a screen taking a run over from another holds it again at once,
    /// and the task (and its Live Activity) goes on.
    func release() {
        guard holders > 0 else { return }
        holders -= 1
        guard holders == 0 else { return }
        letGo?.cancel()
        letGo = Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            guard let self, !Task.isCancelled, self.holders == 0 else { return }
            self.letGo = nil
            self.end(success: true)
        }
    }

    /// describe says what the run is doing, in the Live Activity.
    func describe(title: String, subtitle: String) {
        guard title != self.title || subtitle != self.subtitle else { return }
        self.title = title
        self.subtitle = subtitle
        task?.updateTitle(title, subtitle: subtitle)
    }

    /// cameToFront begins the task anew for the runs still going on, when the
    /// system ended it or its time ran out while the app was away.
    func cameToFront() {
        guard holders > 0, task == nil, submitted == nil else { return }
        begin()
    }

    private func begin() {
        guard BackgroundSettings.keepAlive, task == nil, submitted == nil else { return }
        if fallback?.active != true { fallback = BackgroundTime("kou-run") }
        // An identifier of its own every time: the system kills an app that
        // registers one twice.
        let id = "\(Self.prefix).\(UUID().uuidString)"
        let registered = BGTaskScheduler.shared.register(forTaskWithIdentifier: id, using: .main) { task in
            MainActor.assumeIsolated { RunKeeper.shared.launched(task) }
        }
        guard registered else { return }
        let request = BGContinuedProcessingTaskRequest(identifier: id, title: title, subtitle: subtitle)
        // Now or not at all: queued for later, it is no use to a live stream.
        request.strategy = .fail
        do {
            try BGTaskScheduler.shared.submit(request)
            submitted = id
        } catch {
            // Not now (ImmediateRunIneligible), or not here (Unavailable in the
            // Simulator): the plain background time is all there is.
        }
    }

    private func launched(_ task: BGTask) {
        guard let continued = task as? BGContinuedProcessingTask, task.identifier == submitted,
              holders > 0, BackgroundSettings.keepAlive else {
            // One asked for before, or one no run needs any more.
            task.setTaskCompleted(success: true)
            return
        }
        submitted = nil
        self.task = continued
        if continued.title != title || continued.subtitle != subtitle {
            continued.updateTitle(title, subtitle: subtitle)
        }
        // The task must report progress, or it is taken for stalled and
        // ended: what it shows is the time it may still hold the app.
        let total = Self.cap.components.seconds
        continued.progress.totalUnitCount = total
        continued.progress.completedUnitCount = 0
        let id = task.identifier
        continued.expirationHandler = {
            Task { @MainActor in RunKeeper.shared.expired(id) }
        }
        let start = ContinuousClock.now
        ticker = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                guard let self, !Task.isCancelled else { return }
                let elapsed = ContinuousClock.now - start
                if elapsed >= Self.cap {
                    self.end(success: true)
                    return
                }
                self.task?.progress.completedUnitCount = min(elapsed.components.seconds, total - 1)
            }
        }
        // The continued task runs: the plain background time is not needed.
        fallback?.end()
        fallback = nil
    }

    private func expired(_ id: String) {
        guard task?.identifier == id else { return }
        end(success: false)
    }

    /// end gives the task back; runs still going on begin it anew in front.
    private func end(success: Bool) {
        ticker?.cancel()
        ticker = nil
        if let task {
            task.progress.completedUnitCount = task.progress.totalUnitCount
            task.setTaskCompleted(success: success)
        }
        task = nil
        submitted = nil
        fallback?.end()
        fallback = nil
    }
}

/// TurnNotifier tells the user a turn ended while they were away, with a local
/// notification (the app's own: this build has no push). Tapping one opens the
/// session; no banner is shown on the session's own screen in front.
@MainActor
final class TurnNotifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = TurnNotifier()

    /// The session whose screen is in front, so its turn-end banner is
    /// skipped: the screen shows the end already, and feels it.
    var visibleSession: String?
    /// Opens a session a tapped notification asks for; set once the app is up.
    var open: ((_ workspace: String, _ session: String, _ title: String?) -> Void)? {
        didSet { flush() }
    }
    /// A tap that came before `open` was set (a launch from a tap). Its
    /// title is empty, not nil, when the notification carried none.
    private var pendingTap: (workspace: String, session: String, title: String)?

    /// install takes the notifications' taps: it is to be called while the
    /// app launches, for the tap that launched it.
    func install() {
        UNUserNotificationCenter.current().delegate = self
    }

    /// ask asks once to be allowed to notify, the first time a run starts.
    func ask() async {
        guard BackgroundSettings.notify else { return }
        let center = UNUserNotificationCenter.current()
        guard await center.notificationSettings().authorizationStatus == .notDetermined else { return }
        _ = try? await center.requestAuthorization(options: [.alert, .sound])
    }

    /// post shows a turn-end notification for a session.
    func post(workspace: String, session: String, title: String?, body: String) {
        guard BackgroundSettings.notify else { return }
        let content = UNMutableNotificationContent()
        content.title = title.flatMap { $0.isEmpty ? nil : $0 } ?? "kou-conveyor"
        content.body = body
        content.sound = .default
        content.threadIdentifier = session
        content.userInfo = ["workspace": workspace, "session": session, "title": title ?? ""]
        let request = UNNotificationRequest(identifier: "turn.\(session).\(UUID().uuidString)",
                                            content: content, trigger: nil)
        // The run that ended may let the app go: it stays until the
        // notification is handed over.
        let time = BackgroundTime("kou-notify")
        Task {
            try? await UNUserNotificationCenter.current().add(request)
            time.end()
        }
    }

    private func flush() {
        guard let tap = pendingTap, let open else { return }
        pendingTap = nil
        open(tap.workspace, tap.session, tap.title.isEmpty ? nil : tap.title)
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        let session = notification.request.content.userInfo["session"] as? String
        let visible = await MainActor.run { TurnNotifier.shared.visibleSession }
        // On the very session's screen in front, the screen already shows it.
        if let session, session == visible { return [] }
        return [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        let info = response.notification.request.content.userInfo
        guard let workspace = info["workspace"] as? String, let session = info["session"] as? String else { return }
        let raw = info["title"] as? String ?? ""
        let title = raw.isEmpty ? nil : raw
        await MainActor.run {
            let notifier = TurnNotifier.shared
            if let open = notifier.open {
                open(workspace, session, title)
            } else {
                notifier.pendingTap = (workspace, session, raw)
            }
        }
    }
}
