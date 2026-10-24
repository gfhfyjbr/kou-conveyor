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
                // As the cockpit's tree: the folders first, then the files.
                let entries = listing?.entries ?? []
                let folders = entries.filter(\.isDirectory)
                let files = entries.filter { !$0.isDirectory }
                if !folders.isEmpty {
                    Section {
                        ForEach(folders) { entryRow($0) }
                    } header: {
                        SectionLabel("Folders", count: folders.count)
                            .kouHeader()
                    }
                }
                if !files.isEmpty {
                    Section {
                        ForEach(files) { entryRow($0) }
                    } header: {
                        SectionLabel("Files", count: files.count)
                            .kouHeader(top: folders.isEmpty ? 14 : 22)
                    }
                }
                if listing?.truncated == true {
                    Text("The folder has more entries than are shown.")
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg4)
                        .listRowBackground(Color.kouGround)
                        .listRowSeparator(.hidden)
                }
            } else if let matches, !matches.isEmpty {
                Section {
                    ForEach(matches) { match in
                        let target = match.path.hasSuffix("/") ? String(match.path.dropLast()) : match.path
                        NavigationLink(value: FileRoute(workspace: workspace, path: target, directory: match.directory == true)) {
                            FileRow(name: (target as NSString).lastPathComponent,
                                    directory: match.directory == true,
                                    parent: (target as NSString).deletingLastPathComponent)
                        }
                        .fileRow()
                    }
                } header: {
                    SectionLabel("Found", count: matches.count)
                        .kouHeader()
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color.kouGround)
        .overlay {
            if query.isEmpty {
                if listing == nil {
                    if let error {
                        KouEmpty(symbol: "folder.badge.questionmark", title: "Folder unavailable",
                                 message: error, tone: .kouErr)
                    } else {
                        LoadingSpinner()
                    }
                } else if listing?.entries?.isEmpty ?? true {
                    KouEmpty(symbol: "folder", title: "Empty folder",
                             message: "Nothing in it yet, or only what git ignores and the cockpit hides.")
                }
            } else if matches?.isEmpty == true {
                KouEmpty(symbol: "magnifyingglass", title: "Nothing found",
                         message: "No file or folder of the workspace matches “\(query)”.")
            }
        }
        .searchable(text: $query, prompt: "Find a file")
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        .task(id: query) { await search() }
        .task { await load() }
        .refreshable { await load() }
        .navigationTitle(title ?? (path as NSString).lastPathComponent)
        .navigationSubtitle(subtitle)
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
                    .tint(Color.kouFg)
                }
            }
        }
    }

    /// Where the folder is: the workspace's folder at the top, and below it
    /// the folders it is in — the title says its own name.
    private var subtitle: String {
        if path.isEmpty { return app.workspace?.display ?? app.workspace?.path ?? "" }
        let parent = (path as NSString).deletingLastPathComponent
        let root = app.workspace?.name ?? ""
        return parent.isEmpty ? root : root.isEmpty ? parent : root + "/" + parent
    }

    @ViewBuilder
    private func entryRow(_ entry: TreeEntry) -> some View {
        let target = path.isEmpty ? entry.name : path + "/" + entry.name
        let label = FileRow(name: entry.name, directory: entry.isDirectory, link: entry.link == true,
                            git: entry.git, changed: entry.changed == true,
                            size: entry.isDirectory ? nil : entry.size)
            .opacity(entry.ignored == true ? 0.5 : 1)
        if entry.broken == true {
            label
                .opacity(0.5)
                .fileRow()
        } else {
            NavigationLink(value: FileRoute(workspace: workspace, path: target, directory: entry.isDirectory)) {
                label
            }
            .fileRow()
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

/// GitBadge is a file's state in git — M, A, D, R, ? — as the cockpit's
/// tree marks it: the bare letter, in the colour of what happened.
struct GitBadge: View {
    let state: String

    var body: some View {
        Text(verbatim: state)
            .font(.system(size: 11, weight: .heavy, design: .monospaced))
            .foregroundStyle(Self.color(state))
            .frame(minWidth: 14)
            .accessibilityLabel("Git: \(Self.word(state))")
    }

    /// The colour of a state, for the mark and the name both.
    static func color(_ state: String?) -> Color {
        switch state?.first {
        case "M", "T": .kouWarn
        case "A", "?": .kouOK
        case "D", "U": .kouErr
        case "R", "C": .kouAccent
        default: .kouFg4
        }
    }

    private static func word(_ state: String) -> String {
        switch state.first {
        case "M": "modified"
        case "A": "added"
        case "D": "deleted"
        case "R": "renamed"
        case "C": "copied"
        case "U": "conflict"
        case "?": "untracked"
        case "T": "type changed"
        default: state
        }
    }
}

/// FileRow is a folder or a file as the tree lists it: its kind's mark, its
/// name in mono — coloured as git sees it —, and what git and its size say.
struct FileRow: View {
    let name: String
    let directory: Bool
    var link = false
    var git: String?
    /// Something in the folder changed.
    var changed = false
    var size: Int64?
    /// The folder it is in, for a search's results.
    var parent: String?

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: directory ? "folder.fill" : FileIcon.symbol(name))
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(directory ? Color.kouFg3 : FileKind.color(name))
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(verbatim: name)
                        .font(.system(.subheadline, design: .monospaced, weight: directory ? .semibold : .regular))
                        .foregroundStyle(nameColor)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    if link {
                        Image(systemName: "arrow.turn.up.right")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(Color.kouFg4)
                            .accessibilityLabel("Link")
                    }
                }
                if let parent, !parent.isEmpty {
                    Text(verbatim: parent)
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg4)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
            }
            Spacer(minLength: 8)
            if let git, !git.isEmpty {
                GitBadge(state: git)
            } else if changed {
                Text(verbatim: "•")
                    .font(.system(size: 15, weight: .heavy, design: .monospaced))
                    .foregroundStyle(Color.kouWarn)
                    .accessibilityLabel("Changed inside")
            }
            if let size {
                Text(verbatim: Format.bytes(size))
                    .font(.kouMeta)
                    .monospacedDigit()
                    .foregroundStyle(Color.kouFg4)
            }
            if directory {
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Color.kouFg4)
            }
        }
        .contentShape(.rect)
    }

    /// As the cockpit: a modified file's name in the warning's colour, an
    /// added one's in the good one's, a deleted one's in red.
    private var nameColor: Color {
        guard let git, !git.isEmpty, git.first != "R", git.first != "C" else {
            return directory ? .kouFg : .kouFg2
        }
        return GitBadge.color(git)
    }
}

extension View {
    /// fileRow is a row of a folder's list: on the ground, between
    /// hairlines, without the link's chevron — a folder draws its own.
    func fileRow() -> some View {
        navigationLinkIndicatorVisibility(.hidden)
            .listRowBackground(Color.kouGround)
            .listRowSeparatorTint(Color.kouLine)
            .listRowInsets(EdgeInsets(top: 11, leading: 16, bottom: 11, trailing: 18))
            .alignmentGuide(.listRowSeparatorLeading) { _ in 36 }
    }
}

/// FileKind is the colour of a kind of file, as the cockpit's tree marks
/// it: Go teal, JavaScript yellow, data green…
enum FileKind {
    private static let kinds: [(extensions: [String], color: Color)] = [
        (["go", "mod", "sum"], tone("#5fb3c4", "#2f8496")),
        (["js", "mjs", "cjs", "jsx"], tone("#e0c057", "#a07f12")),
        (["ts", "tsx", "mts"], tone("#5b8fd6", "#2f66b3")),
        (["css", "scss", "less"], tone("#b07fd8", "#8549b5")),
        (["html", "htm", "svg", "xml"], tone("#e0784f", "#c0532a")),
        (["md", "mdx", "txt", "rst"], .kouFg3),
        (["json", "yaml", "yml", "toml", "ini", "env", "conf"], tone("#8fb573", "#5b8a3f")),
        (["sh", "bash", "zsh", "fish"], tone("#9ccf83", "#4f8f35")),
        (["py", "rb", "php", "pl", "lua"], tone("#6f9ad6", "#3f6fb5")),
        (["rs", "c", "h", "cc", "cpp", "hpp", "zig", "swift", "kt", "java", "cs"], tone("#d68a5b", "#b0612f")),
        (["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "avif"], tone("#d28bc4", "#b0559e")),
        (["lock"], .kouFg4),
    ]

    static func color(_ name: String) -> Color {
        let lower = name.lowercased()
        if lower == "makefile" || lower == "dockerfile" {
            return kinds[7].color
        }
        // A dot file, as .env, is its own extension, as in the cockpit.
        guard let dot = lower.lastIndex(of: ".") else { return .kouFg3 }
        let ext = String(lower[lower.index(after: dot)...])
        return kinds.first { $0.extensions.contains(ext) }?.color ?? .kouFg3
    }

    private static func tone(_ dark: String, _ light: String) -> Color {
        Color(uiColor: Look.dynamic(light: light, dark: dark))
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
                KouEmpty(symbol: "doc.badge.ellipsis", title: "Too large to show",
                         message: "\(Format.bytes(file.size)): open it on the computer the cockpit runs on.",
                         tone: .kouWarn)
            } else if let file, file.binary == true {
                KouEmpty(symbol: "doc.badge.gearshape", title: "Binary file",
                         message: "\(Format.bytes(file.size)) of data that is not text.")
            } else if let text {
                CodeText(text: text)
                    .ignoresSafeArea(edges: .bottom)
            } else if file == nil, let error {
                KouEmpty(symbol: "doc.questionmark", title: "File unavailable", message: error, tone: .kouErr)
            } else {
                // One spinner while the file comes and while it is
                // highlighted: two, the one shown went as the file came,
                // and the other only showed after its own wait.
                LoadingSpinner()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.kouGround)
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
                .tint(Color.kouFg)
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
