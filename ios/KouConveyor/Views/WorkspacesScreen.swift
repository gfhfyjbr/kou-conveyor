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
            } header: {
                SectionLabel("Open in the cockpit", count: workspaces.count)
                    .kouHeader()
            } footer: {
                Text("Workspaces are opened and closed in the cockpit on your computer.")
                    .font(.footnote)
                    .foregroundStyle(Color.kouFg4)
                    .padding(.top, 10)
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color.kouGround)
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
        .navigationLinkIndicatorVisibility(.hidden)
        .listRowBackground(Color.kouGround)
        .listRowSeparatorTint(Color.kouLine)
        .listRowInsets(EdgeInsets(top: 14, leading: 16, bottom: 14, trailing: 18))
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

/// WorkspaceRow is a workspace as its sessions are listed: its monogram,
/// when it was last written to, its latest session, how many run and where
/// it is.
private struct WorkspaceRow: View {
    let workspace: Workspace
    let summary: WorkspaceSummary?
    let unavailable: Bool
    /// Whether it is the workspace the app shows.
    let current: Bool

    private var running: Int { summary?.running ?? workspace.running ?? 0 }

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            Monogram(name: workspace.name, size: 44, current: current)
                .padding(current ? 0 : 3)
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(verbatim: workspace.name)
                        .font(.system(.headline, design: .default, weight: .bold).width(.expanded))
                        .foregroundStyle(Color.kouFg)
                        .lineLimit(1)
                    if current {
                        Tag("shown", tone: .kouAccent)
                    }
                    Spacer(minLength: 0)
                    if let date = summary?.lastActive {
                        // Read again each minute: "now" turns "2m".
                        TimelineView(.everyMinute) { _ in
                            Text(verbatim: date.kouAge)
                        }
                        .font(.kouMeta)
                        .foregroundStyle(running > 0 ? Color.kouAccent : Color.kouFg4)
                    }
                }
                latest
                    .font(.subheadline)
                    .foregroundStyle(Color.kouFg2)
                    .lineLimit(1)
                details
                    .padding(.top, 2)
            }
            // The row's separator starts under its text, not under its mark.
            .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
        }
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(current ? .isSelected : [])
    }

    /// The session written to last, for what goes on in the workspace.
    @ViewBuilder private var latest: some View {
        if let summary {
            if let session = summary.latest {
                Text(verbatim: session.title.isEmpty ? "Untitled" : session.title)
            } else {
                Text("No sessions yet")
                    .foregroundStyle(Color.kouFg4)
            }
        } else if unavailable {
            Label("Sessions unavailable", systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(Color.kouWarn)
        } else {
            // Its shape, while the sessions are on their way.
            Text("The session written to last")
                .redacted(reason: .placeholder)
        }
    }

    private var details: some View {
        HStack(spacing: 8) {
            if running > 0 {
                HStack(spacing: 2) {
                    LiveDot(size: 6)
                    Eyebrow("\(running) running", color: .kouAccent)
                }
                .fixedSize()
            } else if let summary {
                Eyebrow(summary.sessions == 1 ? "1 session" : "\(summary.sessions) sessions", color: .kouFg3)
                    .fixedSize()
            }
            Text(verbatim: workspace.display ?? workspace.path)
                .font(.kouMeta)
                .foregroundStyle(Color.kouFg4)
                .lineLimit(1)
                .truncationMode(.head)
        }
    }
}
