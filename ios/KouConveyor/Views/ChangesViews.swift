import SwiftUI
import UIKit

/// ChangesRoute is a step into what a session's prompts changed.
enum ChangesRoute: Hashable {
    /// The files a prompt changed.
    case prompt(message: String, text: String)
    /// The diff of one of them.
    case diff(message: String, file: FileChange)
}

/// ChangesSheet shows what a session's prompts changed in the workspace, as
/// the cockpit's Changes panel does: the prompts, the files one changed, the
/// diff of a file.
struct ChangesSheet: View {
    let client: RelayClient
    let workspace: String
    let session: String
    let prompts: [Entry]
    @Environment(\.dismiss) private var dismiss
    @State private var path: [ChangesRoute]

    init(client: RelayClient, workspace: String, session: String, prompts: [Entry], start: Entry?) {
        self.client = client
        self.workspace = workspace
        self.session = session
        self.prompts = prompts
        let first = start.flatMap { entry in
            entry.messageID.map { ChangesRoute.prompt(message: $0, text: entry.text ?? "") }
        }
        _path = State(initialValue: first.map { [$0] } ?? [])
    }

    var body: some View {
        NavigationStack(path: $path) {
            List {
                ForEach(prompts.reversed()) { prompt in
                    if let message = prompt.messageID {
                        NavigationLink(value: ChangesRoute.prompt(message: message, text: prompt.text ?? "")) {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(prompt.text ?? "")
                                    .lineLimit(2)
                                if let at = prompt.at {
                                    Text(at, format: .relative(presentation: .named))
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                            }
                        }
                    }
                }
            }
            .overlay {
                if prompts.isEmpty {
                    ContentUnavailableView("No prompts yet", systemImage: "plusminus.circle",
                                           description: Text("What each prompt changes in the workspace shows here."))
                }
            }
            .navigationTitle("Changes")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
            }
            .navigationDestination(for: ChangesRoute.self) { route in
                switch route {
                case let .prompt(message, text):
                    ChangesScreen(client: client, workspace: workspace, session: session, message: message, text: text)
                case let .diff(message, file):
                    DiffScreen(client: client, workspace: workspace, session: session, message: message, file: file)
                }
            }
        }
    }
}

/// ChangesScreen is what one prompt changed: its files, with the lines each
/// gained and lost. While the prompt's run goes on, it follows them.
struct ChangesScreen: View {
    let client: RelayClient
    let workspace: String
    let session: String
    let message: String
    let text: String
    @State private var changes: ChangesView?
    @State private var error: String?

    var body: some View {
        List {
            Section {
                Text(text)
                    .font(.subheadline)
                    .lineLimit(4)
                if let changes, changes.available == true, let stat = changes.stat {
                    HStack(spacing: 10) {
                        Text("\(stat.files ?? 0) file\(stat.files == 1 ? "" : "s")")
                        Text("+\(stat.added ?? 0)")
                            .foregroundStyle(.green)
                        Text("−\(stat.removed ?? 0)")
                            .foregroundStyle(.red)
                        Spacer(minLength: 0)
                        if changes.live == true {
                            Label("Running", systemImage: "circle.dotted")
                                .foregroundStyle(Color.kouAccent)
                        }
                    }
                    .font(.footnote.monospacedDigit())
                }
            }
            if let changes, changes.available == true, let files = changes.files, !files.isEmpty {
                Section("Files") {
                    ForEach(files) { file in
                        NavigationLink(value: ChangesRoute.diff(message: message, file: file)) {
                            FileChangeRow(file: file)
                        }
                    }
                }
            }
        }
        .overlay {
            if let changes {
                if changes.available != true {
                    ContentUnavailableView("No changes", systemImage: "plusminus.circle",
                                           description: Text(changes.reason ?? "No changes were recorded for this prompt."))
                } else if changes.files?.isEmpty ?? true {
                    ContentUnavailableView("No changes", systemImage: "checkmark.circle",
                                           description: Text(changes.live == true ? "The run goes on: what it changes shows here." : "This prompt changed no files."))
                }
            } else if let error {
                ContentUnavailableView("Changes unavailable", systemImage: "exclamationmark.triangle", description: Text(error))
            } else {
                ProgressView()
            }
        }
        .refreshable { await load() }
        .task { await follow() }
        .navigationTitle("Changes")
        .navigationBarTitleDisplayMode(.inline)
    }

    private func load() async {
        do {
            let changes: ChangesView = try await client.get(API.workspace(workspace, "sessions", session, "changes", message))
            self.changes = changes
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// follow asks again while the prompt's run goes on.
    private func follow() async {
        repeat {
            await load()
            guard changes?.live == true else { return }
            try? await Task.sleep(for: .seconds(3))
        } while !Task.isCancelled
    }
}

struct FileChangeRow: View {
    let file: FileChange

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(tint)
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 2) {
                Text((file.path as NSString).lastPathComponent)
                    .font(.subheadline.weight(.medium))
                    .lineLimit(1)
                if !detail.isEmpty {
                    Text(detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
            }
            Spacer(minLength: 4)
            if file.binary == true {
                Text("binary")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                HStack(spacing: 6) {
                    if let added = file.added, added > 0 {
                        Text("+\(added)").foregroundStyle(.green)
                    }
                    if let removed = file.removed, removed > 0 {
                        Text("−\(removed)").foregroundStyle(.red)
                    }
                }
                .font(.caption.monospacedDigit())
            }
        }
    }

    private var detail: String {
        if let old = file.oldPath, !old.isEmpty, old != file.path { return "\(old) → \(file.path)" }
        return (file.path as NSString).deletingLastPathComponent
    }

    private var symbol: String {
        switch file.status ?? "" {
        case "added": "plus.square.fill"
        case "deleted": "minus.square.fill"
        case "renamed": "arrow.right.square.fill"
        default: "square.and.pencil"
        }
    }

    private var tint: Color {
        switch file.status ?? "" {
        case "added": .green
        case "deleted": .red
        case "renamed": .blue
        default: .orange
        }
    }
}

/// DiffScreen is the diff of one file a prompt changed.
struct DiffScreen: View {
    let client: RelayClient
    let workspace: String
    let session: String
    let message: String
    let file: FileChange
    @State private var diff: DiffView?
    @State private var text: NSAttributedString?
    @State private var error: String?

    var body: some View {
        Group {
            if let text {
                CodeText(text: text)
                    .ignoresSafeArea(edges: .bottom)
            } else if file.binary == true {
                ContentUnavailableView("Binary file", systemImage: "doc.badge.gearshape",
                                       description: Text("There is no text diff to show."))
            } else if let error {
                ContentUnavailableView("Diff unavailable", systemImage: "exclamationmark.triangle", description: Text(error))
            } else {
                ProgressView()
            }
        }
        .safeAreaBar(edge: .top) {
            if diff?.truncated == true {
                Label("The diff is cut short: it is too long to show whole.", systemImage: "scissors")
                    .font(.footnote)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .glassEffect(.regular, in: .capsule)
            }
        }
        .navigationTitle((file.path as NSString).lastPathComponent)
        .navigationBarTitleDisplayMode(.inline)
        .navigationSubtitle(file.path)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("Copy the diff", systemImage: "doc.on.doc") { UIPasteboard.general.string = diff?.patch }
                        .disabled(diff?.patch?.isEmpty ?? true)
                    Button("Copy the path", systemImage: "link") { UIPasteboard.general.string = file.path }
                } label: {
                    Label("More", systemImage: "ellipsis")
                }
            }
        }
        .task { await load() }
    }

    private func load() async {
        guard file.binary != true else { return }
        var query = [URLQueryItem(name: "path", value: file.path)]
        if let old = file.oldPath, !old.isEmpty { query.append(URLQueryItem(name: "old", value: old)) }
        do {
            let diff: DiffView = try await client.get(API.workspace(workspace, "sessions", session, "changes", message, "diff"),
                                                      query: query)
            self.diff = diff
            let patch = diff.patch ?? ""
            text = patch.isEmpty ? Highlight.plain("The file's text did not change.") : Highlight.diff(patch)
        } catch is CancellationError {
        } catch {
            self.error = error.localizedDescription
        }
    }
}
