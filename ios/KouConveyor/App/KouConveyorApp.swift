import SwiftUI

@main
struct KouConveyorApp: App {
    @State private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    init() {
        // Taken while the app launches, for the tap on a turn-end
        // notification that launched it.
        TurnNotifier.shared.install()
        // The large titles wide and heavy, as an instrument's: a workspace's
        // name reads at a glance.
        UINavigationBar.appearance().largeTitleTextAttributes = [
            .font: UIFont.systemFont(ofSize: 32, weight: .heavy, width: .expanded),
        ]
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .onOpenURL { model.open($0) }
                .task {
                    // A tapped turn-end notification opens its session, once
                    // the app is up.
                    TurnNotifier.shared.open = { workspace, session, title in
                        model.openSession(workspace: workspace, id: session, title: title)
                    }
                }
        }
        .onChange(of: scenePhase) {
            AppFront.update(active: scenePhase == .active)
        }
    }
}

/// RootView pairs the app with a relay, then shows the cockpit behind it.
struct RootView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var colorScheme
    /// Whether what streams of the agent's answer and thinking moves as it
    /// comes (the relay screen's setting), for every screen that shows it.
    @AppStorage(StreamSettings.motionKey) private var streamMotion = true

    var body: some View {
        @Bindable var model = model
        Group {
            if model.isPaired {
                CockpitView()
                    .id(model.generation)
            } else {
                PairingView()
            }
        }
        .animation(.smooth, value: model.isPaired)
        .environment(\.streamMotion, streamMotion)
        .environment(\.kouScheme, colorScheme)
        // The accent chosen (Look), here and in what UIKit shows: alerts,
        // menus, the share sheet.
        .tint(Color.kouAccent)
        .onChange(of: Look.shared.accent, initial: true) {
            for case let scene as UIWindowScene in UIApplication.shared.connectedScenes {
                for window in scene.windows {
                    window.tintColor = Look.shared.accentUIColor
                }
            }
        }
        .alert(incoming.title, isPresented: Binding(
            get: { model.incomingLink != nil },
            set: { if !$0 { model.incomingLink = nil } }
        )) {
            Button("Pair") {
                if let link = model.incomingLink {
                    Task { await model.pairFromLink(link) }
                }
                model.incomingLink = nil
            }
            Button("Cancel", role: .cancel) { model.incomingLink = nil }
        } message: {
            Text(incoming.message)
        }
        .alert("Pairing failed", isPresented: Binding(
            get: { model.linkError != nil },
            set: { if !$0 { model.linkError = nil } }
        )) {
            Button("OK", role: .cancel) { model.linkError = nil }
        } message: {
            Text(model.linkError ?? "")
        }
    }

    /// What to ask of a pairing link opened while paired: the same relay set
    /// up anew (a new token or certificate) or another relay.
    private var incoming: (title: String, message: String) {
        let current = model.relay?.address ?? "a relay"
        guard let text = model.incomingLink, let config = try? RelayConfig(link: text) else {
            return ("Pair with another relay?", "This iPhone is paired with \(current).")
        }
        if config.address == model.relay?.address {
            return ("Pair again?",
                    "The link is for \(current), with a new token or certificate: the relay was set up anew. Pairing again replaces the old ones.")
        }
        return ("Pair with another relay?",
                "This iPhone is paired with \(current). The link is for \(config.address): pairing with it forgets \(current).")
    }
}

/// CockpitView is the cockpit through its relay: sessions, canvases, files,
/// accounts and the relay itself. The tunnel says nothing while it carries;
/// down, a strip over every screen says so, and the Relay tab is marked.
struct CockpitView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    /// The tunnel's trouble, once it has lasted: a blip is not shown.
    @State private var trouble: TunnelState?

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            if let trouble {
                LinkStrip(state: trouble)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
            TabView(selection: $model.tab) {
                Tab("Sessions", systemImage: "text.bubble", value: AppModel.Tab.sessions) {
                    SessionsScreen()
                }
                Tab("Canvases", systemImage: "point.3.connected.trianglepath.dotted", value: AppModel.Tab.canvases) {
                    CanvasesScreen()
                }
                Tab("Files", systemImage: "folder", value: AppModel.Tab.files) {
                    FilesScreen()
                }
                Tab("Accounts", systemImage: "person.2", value: AppModel.Tab.accounts) {
                    AccountsScreen()
                }
                Tab("Relay", systemImage: "antenna.radiowaves.left.and.right", value: AppModel.Tab.relay) {
                    RelayScreen()
                }
                .badge(trouble == nil ? nil : Text(verbatim: "!"))
            }
            .tabBarMinimizeBehavior(.onScrollDown)
        }
        .background(Color.kouGround.ignoresSafeArea())
        .environment(\.syntaxClasses, model.syntaxClasses)
        .task(id: scenePhase == .active) {
            guard scenePhase == .active else { return }
            await model.watch()
        }
        .task(id: TunnelState(model.link)) {
            let state = TunnelState(model.link)
            // Trouble shows only where what it keeps away is shown: before,
            // the screens say it whole.
            let shows = (state == .down || state == .cockpitOffline) && model.workspace != nil
            if shows {
                try? await Task.sleep(for: .seconds(1.5))
                guard !Task.isCancelled else { return }
            }
            withAnimation(.spring(response: 0.45, dampingFraction: 0.85)) {
                trouble = shows ? state : nil
            }
        }
    }
}

extension View {
    /// hidesTabBar hides the tab bar, a tab's stack being given, while a
    /// screen is pushed in it: the bar, and the tunnel's accessory over it,
    /// slide away as the screen comes and back as it goes. Hidden by the
    /// pushed screen itself, the bar came back only once the screen was
    /// gone, at once, and the accessory jumped up with it.
    func hidesTabBar(when pushed: Bool) -> some View {
        modifier(TabBarHiding(pushed: pushed))
    }

    /// groupedBackground keeps a list on the grouped gray whether it has
    /// rows or not: empty, a list is white, and its first rows turned the
    /// whole screen gray at once. The gray is the shade chosen (Look).
    func groupedBackground() -> some View {
        scrollContentBackground(.hidden)
            .background(Color.kouGroupedGround)
    }
}

private struct TabBarHiding: ViewModifier {
    let pushed: Bool
    @State private var hidden = false

    func body(content: Content) -> some View {
        content
            .toolbar(hidden ? .hidden : .automatic, for: .tabBar)
            .onChange(of: pushed, initial: true) { _, pushed in
                guard hidden != pushed else { return }
                withAnimation(.smooth(duration: 0.35)) { hidden = pushed }
            }
    }
}

/// A screen of the cockpit that needs it: the cockpit out of reach, or no
/// workspace yet, it says so.
struct CockpitGate<Content: View>: View {
    @Environment(AppModel.self) private var model
    @ViewBuilder var content: (RelayClient, Workspace) -> Content

    var body: some View {
        if let client = model.client, let workspace = model.workspace {
            content(client, workspace)
        } else {
            Group {
                switch model.link {
                case .checking:
                    reaching
                case .cockpitOffline:
                    KouEmpty(symbol: "moon.zzz", title: "The cockpit is asleep",
                             message: "The relay at \(model.relay?.address ?? "") is up, but no cockpit is connected to it. Run /tunnel in the cockpit, or start kou-conveyor-web.",
                             tone: .kouWarn) {
                        retry
                    }
                case let .failed(message):
                    KouEmpty(symbol: "wifi.exclamationmark", title: "The relay is out of reach",
                             message: message, tone: .kouErr) {
                        retry
                    }
                case .online:
                    // The relay answered, its workspaces are a moment behind.
                    if model.workspacesLoaded {
                        KouEmpty(symbol: "folder.badge.questionmark", title: "No workspaces",
                                 message: "The cockpit has no workspace open.")
                    } else {
                        reaching
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            // The ground of the lists that come in its place: on another,
            // the screen changed at once as they came.
            .background(Color.kouGroupedGround)
        }
    }

    private var reaching: some View {
        KouEmpty(symbol: "dot.radiowaves.left.and.right", title: "Reaching the cockpit",
                 message: model.relay?.address, tone: .kouAccent, waiting: true)
    }

    private var retry: some View {
        Button {
            Task { await model.refresh() }
        } label: {
            Label("Try again", systemImage: "arrow.clockwise")
                .font(.subheadline.weight(.semibold))
                .padding(.horizontal, 6)
        }
        .buttonStyle(.glass)
        .buttonBorderShape(.cornerCapsule)
    }
}

/// LoadingSpinner stands for content on its way once the wait is long
/// enough to be noticed: a quick load goes by without a spinner flashing
/// up for a moment, and at launch the cockpit's spinner does not give way
/// to another one, lower down, just before the list comes.
struct LoadingSpinner: View {
    var title: LocalizedStringKey?
    @State private var shown = false

    var body: some View {
        VStack(spacing: 12) {
            ProgressView()
                .tint(Color.kouFg3)
            if let title {
                Text(title)
                    .textCase(.uppercase)
                    .font(.kouEyebrow)
                    .tracking(Font.eyebrowTracking)
                    .foregroundStyle(Color.kouFg3)
            }
        }
        .opacity(shown ? 1 : 0)
        .task {
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            withAnimation(.easeIn(duration: 0.2)) { shown = true }
        }
    }
}
