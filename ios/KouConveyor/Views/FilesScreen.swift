import SwiftUI
import UIKit

/// FileRoute opens a folder or a file of a workspace.
struct FileRoute: Hashable {
    let workspace: String
    let path: String
    let directory: Bool
}

/// FilesScreen is the Files tab: the workspace's folders and files, as the
/// cockpit's explorer shows them, with their state in git.
struct FilesScreen: View {
    @Environment(AppModel.self) private var app
    @State private var path: [FileRoute] = []

    var body: some View {
        NavigationStack(path: $path) {
            CockpitGate { client, workspace in
                DirectoryScreen(client: client, workspace: workspace.id, path: "", title: workspace.name)
                    .id(workspace.id)
            }
            .navigationDestination(for: FileRoute.self) { route in
                if let client = app.client {
                    if route.directory {
                        DirectoryScreen(client: client, workspace: route.workspace, path: route.path, title: nil)
                    } else {
                        FileScreen(client: client, workspace: route.workspace, path: route.path)
                    }
                }
            }
        }
        // What was open is another workspace's.
        .onChange(of: app.workspace?.id) {
            path = []
        }
    }
}

/// DirectoryScreen is a folder of the workspace; its search finds files in
/// the whole workspace.
struct DirectoryScreen: View {
    @Environment(AppModel.self) private var app
    let client: RelayClient
    let workspace: String
    let path: String
    let title: String?
    @State private var listing: TreeListing?
    @State private var error: String?
    @State private var query = ""
    @State private var matches: [FileMatch]?

    var body: some View {
        List {
            if query.isEmpty {
                ForEach(listing?.entries ?? []) { entry in
                    entryRow(entry)
                }
                if listing?.truncated == true {
                    Text("The folder has more entries than are shown.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            } else {
                ForEach(matches ?? []) { match in
                    let target = match.path.hasSuffix("/") ? String(match.path.dropLast()) : match.path
                    NavigationLink(value: FileRoute(workspace: workspace, path: target, directory: match.directory == true)) {
                        HStack(spacing: 10) {
                            Image(systemName: match.directory == true ? "folder.fill" : FileIcon.symbol(target))
                                .foregroundStyle(match.directory == true ? Color.kouAccent : Color.secondary)
                                .frame(width: 24)
                            VStack(alignment: .leading, spacing: 2) {
                                Text((target as NSString).lastPathComponent)
                                    .lineLimit(1)
                                let parent = (target as NSString).deletingLastPathComponent
                                if !parent.isEmpty {
                                    Text(parent)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                        .truncationMode(.head)
                                }
                            }
                        }
                    }
                }
            }
        }
        .groupedBackground()
        .overlay {
            if query.isEmpty {
                if listing == nil {
                    if let error {
                        ContentUnavailableView("Folder unavailable", systemImage: "folder.badge.questionmark", description: Text(error))
                    } else {
                        LoadingSpinner()
                    }
                } else if listing?.entries?.isEmpty ?? true {
                    ContentUnavailableView("Empty folder", systemImage: "folder")
                }
            } else if matches?.isEmpty == true {
                ContentUnavailableView.search(text: query)
            }
        }
        .searchable(text: $query, prompt: "Find a file")
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        .task(id: query) { await search() }
        .task { await load() }
        .refreshable { await load() }
        .navigationTitle(title ?? (path as NSString).lastPathComponent)
        .navigationSubtitle(path.isEmpty ? (app.workspace?.display ?? app.workspace?.path ?? "") : path)
        .toolbar {
            if path.isEmpty {
                ToolbarItem(placement: .topBarLeading) {
                    WorkspaceMenu()
                }
            } else {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button("Copy the path", systemImage: "link") { UIPasteboard.general.string = path }
                    } label: {
                        Label("More", systemImage: "ellipsis")
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func entryRow(_ entry: TreeEntry) -> some View {
        let target = path.isEmpty ? entry.name : path + "/" + entry.name
        let label = HStack(spacing: 10) {
            Image(systemName: entry.isDirectory ? "folder.fill" : FileIcon.symbol(entry.name))
                .foregroundStyle(entry.isDirectory ? Color.kouAccent : Color.secondary)
                .frame(width: 24)
            Text(entry.name)
                .lineLimit(1)
                .truncationMode(.middle)
            if entry.link == true {
                Image(systemName: "arrow.turn.up.right")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 6)
            if let git = entry.git, !git.isEmpty {
                GitBadge(state: git)
            } else if entry.changed == true {
                Circle()
                    .fill(.orange)
                    .frame(width: 6, height: 6)
            }
            if !entry.isDirectory, let size = entry.size {
                Text(Format.bytes(size))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
        .opacity(entry.ignored == true ? 0.55 : 1)
        if entry.broken == true {
            label.opacity(0.5)
        } else {
            NavigationLink(value: FileRoute(workspace: workspace, path: target, directory: entry.isDirectory)) {
                label
            }
        }
    }

    private func load() async {
        do {
            let listing: TreeListing = try await client.get(API.workspace(workspace, "tree"),
                                                            query: [URLQueryItem(name: "path", value: path)])
            self.listing = listing
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func search() async {
        let text = query.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else {
            matches = nil
            return
        }
        try? await Task.sleep(for: .milliseconds(250))
        guard !Task.isCancelled else { return }
        do {
            let found: FileSearch = try await client.get(API.workspace(workspace, "files"),
                                                         query: [URLQueryItem(name: "q", value: text), URLQueryItem(name: "limit", value: "60")])
            matches = found.files ?? []
        } catch is CancellationError {
        } catch {
            matches = []
        }
    }
}

/// GitBadge is a file's state in git: M, A, D, R, ?…
struct GitBadge: View {
    let state: String

    var body: some View {
        Text(state)
            .font(.caption2.weight(.bold).monospaced())
            .foregroundStyle(color)
            .padding(.horizontal, 5)
            .padding(.vertical, 1)
            .background(color.opacity(0.15), in: .cornerCapsule)
            .accessibilityLabel("Git: \(state)")
    }

    private var color: Color {
        switch state.first {
        case "M": .orange
        case "A": .green
        case "D": .red
        case "R", "C": .blue
        case "?", "U": .teal
        default: .secondary
        }
    }
}

/// FileScreen is a file of the workspace: its text, highlighted as the
/// cockpit highlights it, or its picture.
struct FileScreen: View {
    let client: RelayClient
    let workspace: String
    let path: String
    @State private var file: FileView?
    @State private var text: NSAttributedString?
    @State private var error: String?
    @State private var zoomed = false

    var body: some View {
        Group {
            if let file, let image = file.image, !image.isEmpty {
                ScrollView {
                    Button {
                        zoomed = true
                    } label: {
                        RemoteImage(client: client, path: rawPath, query: rawQuery, contentMode: .fit)
                            .frame(maxWidth: .infinity, minHeight: 240)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(name)
                    .padding()
                }
            } else if let file, file.tooLarge == true {
                ContentUnavailableView("Too large to show", systemImage: "doc.badge.ellipsis",
                                       description: Text("\(Format.bytes(file.size)): open it on the computer the cockpit runs on."))
            } else if let file, file.binary == true {
                ContentUnavailableView("Binary file", systemImage: "doc.badge.gearshape",
                                       description: Text("\(Format.bytes(file.size)) of data that is not text."))
            } else if let text {
                CodeText(text: text)
                    .ignoresSafeArea(edges: .bottom)
            } else if file == nil, let error {
                ContentUnavailableView("File unavailable", systemImage: "doc.questionmark", description: Text(error))
            } else {
                // One spinner while the file comes and while it is
                // highlighted: two, the one shown went as the file came,
                // and the other only showed after its own wait.
                LoadingSpinner()
            }
        }
        .navigationTitle(name)
        .navigationBarTitleDisplayMode(.inline)
        .navigationSubtitle(subtitle)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    if let text = file?.text, !text.isEmpty {
                        Button("Copy the text", systemImage: "doc.on.doc") { UIPasteboard.general.string = text }
                    }
                    Button("Copy the path", systemImage: "link") { UIPasteboard.general.string = path }
                    Button("Reload", systemImage: "arrow.clockwise") { Task { await load() } }
                } label: {
                    Label("More", systemImage: "ellipsis")
                }
            }
        }
        .sheet(isPresented: $zoomed) {
            PictureViewer(client: client, path: rawPath, query: rawQuery, title: name)
        }
        .task { await load() }
    }

    private var name: String { (path as NSString).lastPathComponent }
    private var rawPath: String { API.workspace(workspace, "raw") }
    private var rawQuery: [URLQueryItem] { [URLQueryItem(name: "path", value: path)] }

    private var subtitle: String {
        guard let file else { return path }
        var parts: [String] = []
        if let language = file.language, !language.isEmpty { parts.append(language) }
        if file.size != nil { parts.append(Format.bytes(file.size)) }
        if let modified = file.modified { parts.append(modified.formatted(.relative(presentation: .named))) }
        return parts.isEmpty ? path : parts.joined(separator: " · ")
    }

    private func load() async {
        do {
            let file: FileView = try await client.get(API.workspace(workspace, "file"), query: [URLQueryItem(name: "path", value: path)])
            self.file = file
            error = nil
            if let content = file.text {
                let runs = file.runs, classes = file.classes
                let colored = await Highlight.colored { Highlight.attributed(content, runs: runs, classes: classes) }
                guard !Task.isCancelled else { return }
                text = colored.text
            }
        } catch is CancellationError {
        } catch {
            self.error = error.localizedDescription
        }
    }
}

/// FileIcon is the symbol of a kind of file, by its extension.
enum FileIcon {
    static func symbol(_ name: String) -> String {
        switch (name as NSString).pathExtension.lowercased() {
        case "swift", "go", "js", "mjs", "ts", "tsx", "jsx", "py", "rb", "rs", "c", "h", "cc", "cpp", "m", "java", "kt", "cs",
             "php", "lua", "zig":
            "chevron.left.forwardslash.chevron.right"
        case "md", "txt", "rst", "adoc": "doc.text"
        case "json", "yml", "yaml", "toml", "plist", "xml", "ini", "conf": "curlybraces"
        case "png", "jpg", "jpeg", "gif", "webp", "heic", "svg", "ico", "bmp": "photo"
        case "sh", "bash", "zsh", "fish": "terminal"
        case "html", "htm", "css", "scss": "globe"
        case "zip", "gz", "tgz", "tar", "xz", "7z": "doc.zipper"
        case "pdf": "doc.richtext"
        case "mod", "sum", "lock": "lock.doc"
        default: "doc"
        }
    }
}
