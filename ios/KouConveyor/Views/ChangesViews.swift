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
                LoadingSpinner()
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
            await AppFront.poll(.seconds(3))
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
                LoadingSpinner()
            }
        }
        .safeAreaBar(edge: .top) {
            if diff?.truncated == true {
                Label("The diff is cut short: it is too long to show whole.", systemImage: "scissors")
                    .font(.footnote)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .glassEffect(.regular, in: .cornerCapsule)
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
            let colored = await Highlight.colored {
                patch.isEmpty ? Highlight.plain("The file's text did not change.") : Highlight.diff(patch)
            }
            guard !Task.isCancelled else { return }
            text = colored.text
        } catch is CancellationError {
        } catch {
            self.error = error.localizedDescription
        }
    }
}

// MARK: What a Write or an Edit call changed

extension EnvironmentValues {
    /// The names of the classes the cockpit highlights code with, by their
    /// index in the runs of a highlighted text (its config's syntax_classes):
    /// nil until the config is read, and code shows plain.
    @Entry var syntaxClasses: [String]? = nil
}

/// ChangeBlock is what a Write call wrote, its lines numbered, or what an
/// Edit call changed — the lines it took out and put in, among the lines
/// around them, numbered as in the file where the edit said where it was —
/// in the colours of the code's tokens, as the cockpit's timeline shows it.
/// A call that failed shows, dimmed, what it was to do. The transcript shows
/// its start; ChangeSheet, all of it.
///
///     DIFF · +2 −1                             ⧉
///     41 41   func main() {
///     42    -     println(1)
///        42 +     println(2)
///        43 +     println(3)
///     43 44   }
///
/// It is Equatable by what it shows: the call around it draws itself anew
/// as runs start and end, and its rows need not be made again.
struct ChangeBlock: View, @MainActor Equatable {
    let change: Tool.Change
    let failed: Bool
    /// The classes of the colours of the code (syntaxClasses).
    let classes: [String]?
    /// Shows all of it apart; nil where it cannot, and all of it shows here.
    var all: (() -> Void)? = nil
    /// Whether all of it shows here.
    @State private var open = false
    @State private var copied = false
    /// Counts the copies: each one keeps the check a while longer.
    @State private var copies = 0

    /// How many of its lines the transcript shows.
    static let shown = 40

    static func == (a: ChangeBlock, b: ChangeBlock) -> Bool {
        // all does the same from one drawing to the next.
        a.change == b.change && a.failed == b.failed && a.classes == b.classes && (a.all == nil) == (b.all == nil)
    }

    var body: some View {
        let shown = ChangeRows.make(change, classes: classes, limit: open ? .max : Self.shown)
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(Self.label(change, failed: failed))
                    .font(.system(.caption2, design: .monospaced, weight: .semibold))
                    .textCase(.uppercase)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer()
                copyButton
            }
            .padding(.leading, 10)
            .padding(.trailing, 2)
            .padding(.top, 2)
            if shown.rows.isEmpty {
                Text("An empty file")
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .padding(.horizontal, 10)
                    .padding(.bottom, 10)
            } else {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(shown.rows) { row in
                        ChangeRowView(row: row, diff: shown.diff, digits: shown.digits)
                    }
                }
                .font(.system(.caption, design: .monospaced))
                .opacity(failed ? 0.7 : 1)
                .padding(.bottom, shown.hidden > 0 ? 0 : 8)
            }
            if shown.hidden > 0 {
                Button("Show \(shown.hidden) more \(shown.hidden == 1 ? "line" : "lines")") {
                    if let all { all() } else { withAnimation(.snappy) { open = true } }
                }
                .font(.caption.weight(.medium))
                .buttonStyle(.borderless)
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.kouSurface)
        .clipShape(.rect(cornerRadius: .corner(12)))
        .overlay(RoundedRectangle(cornerRadius: .corner(12)).strokeBorder(.separator.opacity(0.6), lineWidth: 0.5))
    }

    private var copyButton: some View {
        Button {
            UIPasteboard.general.string = change.text
            copies += 1
            withAnimation(.snappy) { copied = true }
        } label: {
            Image(systemName: copied ? "checkmark" : "doc.on.doc")
                .contentTransition(.symbolEffect(.replace))
                .font(.caption)
                .frame(width: 32, height: 26)
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .foregroundStyle(copied ? AnyShapeStyle(.green) : AnyShapeStyle(.secondary))
        .accessibilityLabel(copied ? "Copied" : change.kind == "write" ? "Copy the content" : "Copy the diff")
        .sensoryFeedback(.success, trigger: copies)
        .task(id: copies) {
            guard copies > 0 else { return }
            try? await Task.sleep(for: .seconds(1.4))
            guard !Task.isCancelled else { return }
            withAnimation(.snappy) { copied = false }
        }
    }

    /// What a change is, as the page heads it: "Content · 120 lines",
    /// "Diff · +2 −1 · 3 replacements"; what a call that failed was to do is
    /// "Not written", "Not applied".
    static func label(_ change: Tool.Change, failed: Bool) -> String {
        if change.kind == "write" {
            let n = change.lineCount
            return "\(failed ? "Not written" : "Content") · \(n) \(n == 1 ? "line" : "lines")"
        }
        let counts = change.counts
        var parts = [failed ? "Not applied" : "Diff", "+\(counts.added) −\(counts.removed)"]
        if let n = change.replacements, n > 1 { parts.append("\(n) replacements") }
        return parts.joined(separator: " · ")
    }

    /// said is the first line of what a Write or an Edit call said it did,
    /// without what it goes on with: the lines around the change after it,
    /// which the diff shows.
    static func said(_ output: String) -> String {
        let end = output.unicodeScalars.firstIndex(of: "\n") ?? output.endIndex
        var line = String(output[..<end])
        if let tail = line.range(of: #"\s*The file now reads around the first change:\s*$"#, options: .regularExpression) {
            line.removeSubrange(tail)
        }
        return line
    }
}

/// ChangeStat says in the head of a Write or an Edit call how much it
/// changed, as the page does: the lines it put in and took out, or the lines
/// it wrote.
struct ChangeStat: View {
    let change: Tool.Change

    var body: some View {
        if change.kind == "edit", let diff = change.diff, !diff.isEmpty {
            let counts = change.counts
            HStack(spacing: 4) {
                Text(verbatim: "+\(counts.added)")
                    .foregroundStyle(.green)
                Text(verbatim: "−\(counts.removed)")
                    .foregroundStyle(.red)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(counts.added) added, \(counts.removed) removed")
        } else if change.kind == "write" {
            let n = change.lineCount
            Text("\(n) \(n == 1 ? "line" : "lines")")
        }
    }
}

/// ChangeSheet is all of what a Write call wrote, or an Edit call changed:
/// the transcript shows its start.
struct ChangeSheet: View {
    let entry: Entry
    @Environment(\.dismiss) private var dismiss
    @Environment(\.syntaxClasses) private var classes
    @State private var rows: ChangeRows?

    var body: some View {
        NavigationStack {
            Group {
                if let rows, !rows.rows.isEmpty {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(rows.rows) { row in
                                ChangeRowView(row: row, diff: rows.diff, digits: rows.digits)
                            }
                        }
                        .font(.system(.footnote, design: .monospaced))
                        .opacity(failed ? 0.7 : 1)
                        .padding(.vertical, 12)
                    }
                } else if rows != nil {
                    ContentUnavailableView("An empty file", systemImage: "doc")
                } else {
                    LoadingSpinner()
                }
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .navigationSubtitle(change.map { ChangeBlock.label($0, failed: failed) } ?? "")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button(change?.kind == "write" ? "Copy the content" : "Copy the diff", systemImage: "doc.on.doc") {
                            UIPasteboard.general.string = change?.text
                        }
                        .disabled(change?.text?.isEmpty ?? true)
                        if !path.isEmpty {
                            Button("Copy the path", systemImage: "link") { UIPasteboard.general.string = path }
                        }
                    } label: {
                        Label("More", systemImage: "ellipsis")
                    }
                }
            }
        }
        .task(id: classes) { await make() }
    }

    private var change: Tool.Change? { entry.tool?.change }
    private var failed: Bool { entry.tool?.state == "failed" }
    /// The file the call wrote or changed.
    private var path: String { entry.tool?.files?.first ?? ToolSummary.of(entry.tool) }

    private var title: String {
        let name = (path as NSString).lastPathComponent
        return name.isEmpty ? entry.tool?.name ?? "Change" : name
    }

    /// make makes the rows away from the main thread: a long file takes a
    /// while to color.
    private func make() async {
        guard let change else { return }
        let classes = classes
        let made = await Task.detached(priority: .userInitiated) {
            ChangeRows.make(change, classes: classes, limit: .max)
        }.value
        guard !Task.isCancelled else { return }
        rows = made
    }
}

/// ChangeRows are the rows ChangeBlock and ChangeSheet draw of a change: its
/// lines in the colours of their tokens, with their numbers, and between two
/// hunks of a diff, a row for the lines left out.
struct ChangeRows: Sendable {
    struct Row: Identifiable, Sendable {
        enum Kind: Sendable { case line, kept, added, removed, skipped }

        let id: Int
        let kind: Kind
        /// Its numbers in the file before the edit and after; a Write's
        /// line has one, after. Nil where the line has none.
        var before: Int? = nil
        var after: Int? = nil
        /// Its code, or what a row of lines left out says.
        var code = AttributedString()
    }

    var rows: [Row] = []
    /// Whether they are a diff's: a sign before each line, two numbers.
    var diff = false
    /// How wide the numbers are, in digits; 0 when the lines have none.
    var digits = 0
    /// How many lines of the text past the limit do not show.
    var hidden = 0

    /// make makes the rows of the first limit lines of what a change shows
    /// (Tool.Change.text).
    static func make(_ change: Tool.Change, classes: [String]?, limit: Int) -> ChangeRows {
        guard let text = change.text, !text.isEmpty else { return ChangeRows() }
        let edit = change.kind == "edit"
        let total = change.lineCount
        let lines = colored(text, runs: change.syntax, classes: classes, limit: limit, drop: edit ? 1 : 0, total: total)
        var made = ChangeRows(diff: edit, hidden: total - lines.count)
        guard edit else {
            made.rows = lines.enumerated().map { Row(id: $0, kind: .line, after: $0 + 1, code: $1) }
            made.digits = max(2, String(lines.count).count)
            return made
        }
        // The heads of the hunks say where they start: the numbers count
        // from there, the lines taken out before the edit, the lines put in
        // after it.
        let numbered = (change.line ?? 0) > 0
        var before = 0
        var after = 0
        let raw = text.unicodeScalars.split(separator: "\n", maxSplits: lines.count, omittingEmptySubsequences: false)
        for (i, line) in zip(lines.indices, raw) {
            if let hunk = head(Substring(line)) {
                if !made.rows.isEmpty {
                    let skipped = hunk.from - before
                    let says = skipped > 0 ? "⋯ \(skipped) \(skipped == 1 ? "line" : "lines")" : "⋯"
                    made.rows.append(Row(id: made.rows.count, kind: .skipped, code: AttributedString(says)))
                }
                before = hunk.from
                after = hunk.to
                continue
            }
            let sign = line.first
            let kind: Row.Kind = sign == "-" ? .removed : sign == "+" ? .added : .kept
            made.rows.append(Row(id: made.rows.count, kind: kind,
                                 before: numbered && kind != .added ? before : nil,
                                 after: numbered && kind != .removed ? after : nil,
                                 code: lines[i]))
            if kind != .added { before += 1 }
            if kind != .removed { after += 1 }
        }
        made.digits = numbered ? max(2, String(max(before, after) - 1).count) : 0
        return made
    }

    /// head reads the head of a hunk, "@@ -12,3 +12,4 @@": the lines it
    /// starts at before the edit and after.
    static func head(_ line: Substring) -> (from: Int, to: Int)? {
        guard line.hasPrefix("@@ -") else { return nil }
        let parts = line.split(separator: " ", maxSplits: 4)
        guard parts.count >= 4, parts[3] == "@@", parts[2].hasPrefix("+"),
              let from = Int(parts[1].dropFirst().prefix { $0 != "," }),
              let to = Int(parts[2].dropFirst().prefix { $0 != "," })
        else { return nil }
        return (from, to)
    }

    /// colored is a text as its lines, the first limit of them, each in the
    /// colours of its tokens by runs: pairs of a length, in UTF-16 units, and
    /// the index of a class. A token over lines — a comment of many — colors
    /// a piece in each. The first drop units of each line do not show: a
    /// diff's signs. A newline at the end starts no line.
    static func colored(_ text: String, runs: [Int32]?, classes: [String]?, limit: Int, drop: Int, total: Int) -> [AttributedString] {
        // Only the units of the lines that show: the transcript shows the
        // first few lines of a file of thousands.
        var units: [UInt16] = []
        if limit >= total {
            units = Array(text.utf16)
        } else {
            var newlines = 0
            for unit in text.utf16 {
                if unit == 0x0A {
                    newlines += 1
                    if newlines == limit { break }
                }
                units.append(unit)
            }
        }
        let styles = (classes ?? []).map(Style.init)
        var lines = [AttributedString()]
        // How much of the line it is in came before.
        var column = 0
        func put(_ start: Int, _ end: Int, _ style: Style?) {
            var at = start
            while at < end, lines.count <= limit {
                var stop = at
                while stop < end, units[stop] != 0x0A { stop += 1 }
                let from = min(at + max(drop - column, 0), stop)
                column += stop - at
                if from < stop {
                    let part = units[from..<stop]
                    var string = String(decoding: part, as: UTF16.self)
                    // Tabs as wide as they are in the cockpit's code; a
                    // carriage return before a newline breaks no line.
                    if part.contains(where: { $0 == 0x09 || $0 == 0x0D }) {
                        string = string.replacingOccurrences(of: "\t", with: "    ").replacingOccurrences(of: "\r", with: "")
                    }
                    var piece = AttributedString(string)
                    style?.apply(to: &piece)
                    lines[lines.count - 1].append(piece)
                }
                guard stop < end else { return }
                lines.append(AttributedString())
                column = 0
                at = stop + 1
            }
        }
        var at = 0
        if let runs, !styles.isEmpty {
            var i = 0
            while i + 1 < runs.count, at < units.count, lines.count <= limit {
                let length = min(max(Int(runs[i]), 0), units.count - at)
                let index = Int(runs[i + 1])
                put(at, at + length, styles.indices.contains(index) ? styles[index] : nil)
                at += length
                i += 2
            }
        }
        if at < units.count, lines.count <= limit { put(at, units.count, nil) }
        return Array(lines.prefix(min(limit, total)))
    }

    /// How the tokens of a class show: in its colour (Highlight), and bold
    /// for the strong ones.
    private struct Style: Sendable {
        let color: Color?
        let bold: Bool

        init(_ name: String) {
            color = Highlight.color(for: name).map { Color(uiColor: $0) }
            bold = ["gs", "gh", "gu"].contains(name)
        }

        func apply(to piece: inout AttributedString) {
            if let color { piece.foregroundColor = color }
            if bold { piece.inlinePresentationIntent = .stronglyEmphasized }
        }
    }
}

/// ChangeRowView is a row of a change: a line's numbers, its sign in a diff
/// and its code — the lines put in on green, those taken out on red — or the
/// lines between two hunks left out.
private struct ChangeRowView: View {
    let row: ChangeRows.Row
    let diff: Bool
    let digits: Int

    var body: some View {
        if row.kind == .skipped {
            Text(row.code)
                .foregroundStyle(.tertiary)
                .padding(.horizontal, 10)
                .padding(.vertical, 2)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color(uiColor: .tertiarySystemFill))
                .padding(.vertical, 3)
        } else {
            HStack(alignment: .firstTextBaseline, spacing: 0) {
                if digits > 0 {
                    if diff {
                        number(row.before)
                            .padding(.trailing, 6)
                    }
                    number(row.after)
                        .padding(.trailing, diff ? 6 : 10)
                }
                if diff {
                    Text(verbatim: sign)
                        .foregroundStyle(signStyle)
                        .padding(.trailing, 6)
                        .accessibilityHidden(row.kind == .kept)
                }
                Text(row.code)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
            .padding(.horizontal, 10)
            .background(tint)
            .accessibilityElement(children: .combine)
        }
    }

    /// A line's number, right-aligned in the width of the widest.
    private func number(_ n: Int?) -> some View {
        let text = n.map(String.init) ?? ""
        return Text(verbatim: String(repeating: " ", count: max(digits - text.count, 0)) + text)
            .foregroundStyle(.tertiary)
    }

    private var sign: String {
        switch row.kind {
        case .added: "+"
        case .removed: "-"
        default: " "
        }
    }

    private var signStyle: AnyShapeStyle {
        switch row.kind {
        case .added: AnyShapeStyle(.green)
        case .removed: AnyShapeStyle(.red)
        default: AnyShapeStyle(.tertiary)
        }
    }

    private var tint: Color {
        switch row.kind {
        case .added: .green.opacity(0.14)
        case .removed: .red.opacity(0.13)
        default: .clear
        }
    }
}
