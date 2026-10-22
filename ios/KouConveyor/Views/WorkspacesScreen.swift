import SwiftUI
import UIKit

/// WorkspaceList is the cockpit's workspaces, under the sessions of the one
/// shown: a swipe to the right on those brings it. They are listed as the
/// sessions are, each with what goes on in it, the one written to last
/// first.
struct WorkspaceList: View {
    @Environment(AppModel.self) private var app
    let client: RelayClient
    let activity: WorkspacesActivity
    @Binding var path: [SessionsRoute]

    var body: some View {
        let workspaces = ordered
        List {
            Section {
                ForEach(workspaces) { row($0) }
            } footer: {
                Text("Workspaces are opened and closed in the cockpit on your computer.")
            }
        }
        .groupedBackground()
        .animation(.smooth, value: workspaces.map(\.id))
        .refreshable {
            await app.refresh()
            await activity.load(client, app.workspaces)
        }
        .task { await activity.poll(app) }
        .navigationSubtitle(subtitle)
    }

    private func row(_ workspace: Workspace) -> some View {
        let summary = activity.summaries[workspace.id]
        return NavigationLink(value: SessionsRoute.sessions(workspace.id)) {
            WorkspaceRow(workspace: workspace, summary: summary,
                         unavailable: summary == nil && activity.unavailable.contains(workspace.id),
                         current: workspace.id == app.workspace?.id)
        }
        .contextMenu {
            Button("New session", systemImage: "square.and.pencil") { newSession(in: workspace) }
            Button("Copy path", systemImage: "doc.on.doc") { UIPasteboard.general.string = workspace.path }
        }
    }

    private func newSession(in workspace: Workspace) {
        path = [.sessions(workspace.id),
                .session(SessionRoute(workspace: workspace.id, id: UUID().uuidString.lowercased(), fresh: true))]
    }

    /// The workspaces, the one written to last first; those not read yet
    /// after, in the cockpit's order.
    private var ordered: [Workspace] {
        let places = Dictionary(app.workspaces.enumerated().map { ($1.id, $0) }, uniquingKeysWith: { first, _ in first })
        return app.workspaces.sorted { a, b in
            switch (activity.summaries[a.id]?.lastActive, activity.summaries[b.id]?.lastActive) {
            case let (x?, y?) where x != y: x > y
            case (.some, nil): true
            case (nil, .some): false
            default: places[a.id, default: 0] < places[b.id, default: 0]
            }
        }
    }

    private var subtitle: String {
        let count = app.workspaces.count
        let running = app.workspaces.reduce(0) { $0 + (activity.summaries[$1.id]?.running ?? $1.running ?? 0) }
        let place = count == 1 ? "1 workspace" : "\(count) workspaces"
        return running > 0 ? "\(running) running · \(place)" : place
    }
}

/// WorkspaceRow is a workspace as its sessions are listed: its mark, when it
/// was last written to, its latest session, and how many run.
private struct WorkspaceRow: View {
    let workspace: Workspace
    let summary: WorkspaceSummary?
    let unavailable: Bool
    /// Whether it is the workspace the app shows.
    let current: Bool

    private var running: Int { summary?.running ?? workspace.running ?? 0 }

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            WorkspaceTile(workspace: workspace, current: current)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(workspace.name)
                        .font(.body.weight(.semibold))
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    if let date = summary?.lastActive {
                        // Read again each minute: "now" turns "2 min. ago".
                        TimelineView(.everyMinute) { _ in
                            Text(Self.since(date))
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                }
                latest
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                details
                    .font(.caption)
                    .lineLimit(1)
            }
            // The row's separator starts under its text, not under its mark.
            .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(current ? .isSelected : [])
    }

    /// The session written to last, for what goes on in the workspace.
    @ViewBuilder private var latest: some View {
        if let summary {
            if let session = summary.latest {
                Text(session.title.isEmpty ? "Untitled" : session.title)
            } else {
                Text("No sessions yet")
            }
        } else if unavailable {
            Label("Sessions unavailable", systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
        } else {
            // Its shape, while the sessions are on their way.
            Text("The session written to last")
                .redacted(reason: .placeholder)
        }
    }

    private var details: some View {
        HStack(spacing: 4) {
            if running > 0 {
                HStack(spacing: 3) {
                    Image(systemName: "circle.dotted.circle")
                        .symbolEffect(.rotate, options: .repeat(.continuous))
                    Text("\(running) running")
                }
                .foregroundStyle(Color.kouAccent)
                .fixedSize()
            }
            Text(running > 0 ? "· \(facts)" : facts)
                .foregroundStyle(.secondary)
        }
    }

    /// How many sessions it has, and where it is.
    private var facts: String {
        let place = workspace.display ?? workspace.path
        guard let summary else { return place }
        let count = summary.sessions == 1 ? "1 session" : "\(summary.sessions) sessions"
        return "\(count) · \(place)"
    }

    /// "now", "4 min. ago", "2 days ago".
    private static func since(_ date: Date) -> String {
        if Date.now.timeIntervalSince(date) < 60 { return "now" }
        return date.formatted(.relative(presentation: .numeric, unitsStyle: .abbreviated))
    }
}

/// WorkspaceTile is a workspace's mark: the first letter of its name, on a
/// color of its own, and a check on the one the app shows.
private struct WorkspaceTile: View {
    let workspace: Workspace
    let current: Bool

    var body: some View {
        Text(initial)
            .font(.system(size: 19, weight: .semibold, design: .rounded))
            .foregroundStyle(.white)
            .frame(width: 42, height: 42)
            .background(color.gradient, in: .rect(cornerRadius: .corner(11), style: .continuous))
            .overlay(alignment: .bottomTrailing) {
                if current {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 15, weight: .bold))
                        .symbolRenderingMode(.palette)
                        .foregroundStyle(Color.kouAccentInk, Color.kouAccent)
                        // Cut out of the mark by the row's own color.
                        .background(Circle().fill(Color(uiColor: .secondarySystemGroupedBackground)).padding(-2))
                        .offset(x: 5, y: 5)
                }
            }
            .accessibilityHidden(true)
    }

    private var initial: String {
        workspace.name.first { $0.isLetter || $0.isNumber }.map { String($0).uppercased() } ?? "•"
    }

    private static let palette: [Color] = [.orange, .pink, .purple, .indigo, .blue, .teal, .green, .cyan, .red, .brown]

    /// The same for a workspace on every launch: FNV-1a of its ID, where
    /// Swift's own hashing changes from one launch to the next.
    private var color: Color {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in workspace.id.utf8 {
            hash ^= UInt64(byte)
            hash &*= 0x0000_0100_0000_01b3
        }
        return Self.palette[Int(hash % UInt64(Self.palette.count))]
    }
}
