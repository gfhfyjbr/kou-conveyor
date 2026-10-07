import SwiftUI

@main
struct KouConveyorApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .tint(.kouAccent)
                .onOpenURL { model.open($0) }
        }
    }
}

extension Color {
    /// The cockpit's accent, #ff5b1f.
    static let kouAccent = Color(red: 1, green: 91 / 255, blue: 31 / 255)
}

/// RootView pairs the app with a relay, then shows the cockpit behind it.
struct RootView: View {
    @Environment(AppModel.self) private var model

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
        .alert("Pair with another relay?", isPresented: Binding(
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
            Text("This iPhone is paired with \(model.relay?.address ?? "a relay"). The link is of another one.")
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
}

/// CockpitView is the cockpit through its relay: sessions, canvases, and
/// the relay itself; while it is shown, the tunnel's mark floats above the
/// tab bar.
struct CockpitView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        @Bindable var model = model
        TabView(selection: $model.tab) {
            Tab("Sessions", systemImage: "bubble.left.and.text.bubble.right", value: AppModel.Tab.sessions) {
                SessionsScreen()
            }
            Tab("Canvases", systemImage: "point.3.connected.trianglepath.dotted", value: AppModel.Tab.canvases) {
                CanvasesScreen()
            }
            Tab("Relay", systemImage: "globe", value: AppModel.Tab.relay) {
                RelayScreen()
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
        .tabViewBottomAccessory {
            TunnelAccessory()
        }
        .task(id: scenePhase == .active) {
            guard scenePhase == .active else { return }
            await model.watch()
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
            switch model.link {
            case .checking:
                ProgressView("Reaching the cockpit…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .cockpitOffline:
                ContentUnavailableView {
                    Label("The cockpit is not connected", systemImage: "moon.zzz")
                } description: {
                    Text("The relay at \(model.relay?.address ?? "") is up, but no cockpit is connected to it. Run /tunnel in the cockpit, or start kou-conveyor-web.")
                } actions: {
                    Button("Try again") { Task { await model.refresh() } }
                        .buttonStyle(.glass)
                }
            case let .failed(message):
                ContentUnavailableView {
                    Label("The relay is out of reach", systemImage: "wifi.exclamationmark")
                } description: {
                    Text(message)
                } actions: {
                    Button("Try again") { Task { await model.refresh() } }
                        .buttonStyle(.glass)
                }
            case .online:
                ContentUnavailableView("No workspaces", systemImage: "folder.badge.questionmark",
                                       description: Text("The cockpit has no workspace open."))
            }
        }
    }
}
