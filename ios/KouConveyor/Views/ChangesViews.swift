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

    /// A prompt listed: its place in the session, and its message.
    private struct Listed: Identifiable {
        let number: Int
        let entry: Entry
        let message: String
        var id: String { message }
    }

    /// The prompts that changed something or may have, the latest first,
    /// each with its place in the session.
    private var listed: [Listed] {
        prompts.enumerated().reversed().compactMap { n, entry in
            entry.messageID.map { Listed(number: n + 1, entry: entry, message: $0) }
        }
    }

    var body: some View {
        NavigationStack(path: $path) {
            let listed = listed
            List {
                if !listed.isEmpty {
                    Section {
                        ForEach(listed) { item in
                            NavigationLink(value: ChangesRoute.prompt(message: item.message, text: item.entry.text ?? "")) {
                                PromptRow(number: item.number, text: item.entry.text ?? "", at: item.entry.at,
                                          latest: item.message == listed.first?.message)
                            }
                            .listRowBackground(Color.kouGround)
                            .listRowSeparatorTint(Color.kouLine)
                        }
                    } header: {
                        SectionLabel("Prompts", count: listed.count)
                            .kouHeader()
                    }
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(Color.kouGround)
            .overlay {
                if listed.isEmpty {
                    KouEmpty(symbol: "plusminus", title: "No prompts yet",
                             message: "What each prompt changes in the workspace shows here.")
                }
            }
            .navigationTitle("Changes")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
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

/// PromptRow is a prompt of the session as the changes list it: its place
/// in mono — the latest's in the accent, as the cockpit marks it —, its
/// words, and how long ago it was said.
struct PromptRow: View {
    let number: Int
    let text: String
    let at: Date?
    var latest = false

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(verbatim: number < 10 ? "0\(number)" : "\(number)")
                .font(.kouMeta.weight(.semibold))
                .foregroundStyle(latest ? Color.kouAccent : Color.kouFg4)
                .monospacedDigit()
                .frame(minWidth: 18, alignment: .leading)
            Text(verbatim: text)
                .font(.subheadline)
                .foregroundStyle(Color.kouFg)
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
            if let at {
                Text(verbatim: at.kouAge)
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg4)
            }
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
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

    private var files: [FileChange] {
        guard let changes, changes.available == true else { return [] }
        return changes.files ?? []
    }

    var body: some View {
        let files = files
        List {
            Section {
                head
                    .listRowBackground(Color.kouGround)
                    .listRowSeparator(.hidden)
                    .listRowInsets(EdgeInsets(top: 14, leading: 20, bottom: 10, trailing: 20))
                if let quiet {
                    quiet
                        .listRowBackground(Color.kouGround)
                        .listRowSeparator(.hidden)
                }
            }
            if !files.isEmpty {
                Section {
                    ForEach(files) { file in
                        NavigationLink(value: ChangesRoute.diff(message: message, file: file)) {
                            FileChangeRow(file: file)
                        }
                        .listRowBackground(Color.kouGround)
                        .listRowSeparatorTint(Color.kouLine)
                    }
                } header: {
                    SectionLabel("Files", count: files.count)
                        .kouHeader(top: 8)
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color.kouGround)
        .refreshable { await load() }
        .task { await follow() }
        .navigationTitle("Changes")
        .navigationBarTitleDisplayMode(.inline)
    }

    /// The prompt, in the accent the chat says it in, and what it changed
    /// in all as the cockpit's head counts it; while its run goes on, the
    /// accent's blinking square.
    private var head: some View {
        VStack(alignment: .leading, spacing: 14) {
            SectionLabel(title: "Prompt", count: nil, color: .kouFg3) {
                if changes?.live == true {
                    StateMark(word: "Running", tone: .kouAccent, blinking: true)
                }
            }
            Text(verbatim: text)
                .font(.subheadline)
                .foregroundStyle(Color.kouFg)
                .lineLimit(6)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background {
                    ZStack {
                        UserBubble.shape.fill(Color.kouAccent.opacity(0.14))
                        UserBubble.shape.strokeBorder(Color.kouAccent.opacity(0.38), lineWidth: 1)
                    }
                }
            // The counts only once there is something to count: else the
            // empty state below says it.
            if let changes, changes.available == true, let stat = changes.stat, !(changes.files ?? []).isEmpty {
                let added = stat.added ?? 0, removed = stat.removed ?? 0
                ReadingStrip {
                    Reading(label: "Files") {
                        Text(verbatim: "\(stat.files ?? 0)")
                            .foregroundStyle(Color.kouFg)
                    }
                    Reading(label: "Added") {
                        Text(verbatim: "+\(added)")
                            .foregroundStyle(added > 0 ? Color.kouOK : Color.kouFg4)
                    }
                    Reading(label: "Removed") {
                        Text(verbatim: "−\(removed)")
                            .foregroundStyle(removed > 0 ? Color.kouErr : Color.kouFg4)
                    }
                }
                .contentTransition(.numericText())
                .animation(.snappy, value: stat.added)
            }
        }
    }

    /// What shows where the files would: why there are none, or that they
    /// are on their way.
    private var quiet: AnyView? {
        if let changes {
            if changes.available != true {
                return AnyView(KouEmpty(symbol: "plusminus", title: "No changes",
                                        message: changes.reason ?? "No changes were recorded for this prompt."))
            }
            if changes.files?.isEmpty ?? true {
                return AnyView(KouEmpty(symbol: changes.live == true ? "hourglass" : "checkmark", title: "No changes",
                                        message: changes.live == true ? "The run goes on: what it changes shows here." : "This prompt changed no files.",
                                        waiting: changes.live == true))
            }
            return nil
        }
        if let error {
            return AnyView(KouEmpty(symbol: "exclamationmark.triangle", title: "Changes unavailable", message: error, tone: .kouErr))
        }
        return AnyView(LoadingSpinner()
            .frame(maxWidth: .infinity)
            .padding(.vertical, 40))
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

/// FileChangeRow is a file a prompt changed, as the cockpit's tree lists
/// it: its state's letter in its colour, its name in mono — struck through
/// once deleted —, where it is, and the lines it gained and lost.
struct FileChangeRow: View {
    let file: FileChange

    private var deleted: Bool { file.status == "deleted" }

    var body: some View {
        HStack(spacing: 12) {
            Text(verbatim: Self.letter(file.status))
                .font(.system(size: 12, weight: .heavy, design: .monospaced))
                .foregroundStyle(Self.tone(file.status))
                .frame(width: 14)
                .accessibilityLabel(file.status ?? "modified")
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: (file.path as NSString).lastPathComponent)
                    .font(.system(.subheadline, design: .monospaced, weight: .medium))
                    .foregroundStyle(deleted ? Color.kouFg3 : Color.kouFg)
                    .strikethrough(deleted, color: Color.kouFg4)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if !detail.isEmpty {
                    Text(verbatim: detail)
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg3)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
            }
            Spacer(minLength: 6)
            if file.binary == true {
                Tag("Binary", tone: .kouFg3)
            } else {
                VStack(alignment: .trailing, spacing: 5) {
                    DiffCount(added: file.added ?? 0, removed: file.removed ?? 0, zeros: false)
                        .font(.kouMeta)
                    DiffMeter(added: file.added ?? 0, removed: file.removed ?? 0)
                }
            }
        }
        .padding(.vertical, 2)
    }

    private var detail: String {
        if let old = file.oldPath, !old.isEmpty, old != file.path { return "\(old) → \(file.path)" }
        return (file.path as NSString).deletingLastPathComponent
    }

    /// A state's letter, as git and the cockpit say it.
    static func letter(_ status: String?) -> String {
        switch status ?? "" {
        case "added": "A"
        case "deleted": "D"
        case "renamed": "R"
        case "typechange": "T"
        default: "M"
        }
    }

    /// A state's colour, the cockpit's (changes.css .ct-st).
    static func tone(_ status: String?) -> Color {
        switch status ?? "" {
        case "added": .kouOK
        case "deleted": .kouErr
        case "renamed", "typechange": .kouAccent
        default: .kouWarn
        }
    }
}

/// DiffCount is a change's lines as the cockpit counts them: those put in
/// in the success's colour, those taken out in the failure's.
struct DiffCount: View {
    let added: Int
    let removed: Int
    /// Whether a count of nothing shows.
    var zeros = true

    var body: some View {
        HStack(spacing: 5) {
            if zeros || added > 0 {
                Text(verbatim: "+\(added)")
                    .foregroundStyle(Color.kouOK)
            }
            if zeros || removed > 0 {
                Text(verbatim: "−\(removed)")
                    .foregroundStyle(Color.kouErr)
            }
        }
        .monospacedDigit()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(added) added, \(removed) removed")
    }
}

/// DiffMeter is a change's lines as five squares: as many lit as there are
/// lines, up to five, in the success's colour and the failure's by their
/// share — how the change leans, at a glance.
struct DiffMeter: View {
    let added: Int
    let removed: Int

    var body: some View {
        let (gained, lost) = Self.split(added: added, removed: removed)
        HStack(spacing: 2) {
            ForEach(0..<5, id: \.self) { n in
                Rectangle()
                    .fill(n < gained ? Color.kouOK : n < gained + lost ? Color.kouErr : Color.kouLine2)
                    .frame(width: 6, height: 6)
            }
        }
        .accessibilityHidden(true)
    }

    /// How many squares each lights: one at least for each that is not
    /// nothing.
    static func split(added: Int, removed: Int) -> (Int, Int) {
        let total = added + removed
        guard total > 0 else { return (0, 0) }
        let lit = min(total, 5)
        var gained = Int((Double(added) / Double(total) * Double(lit)).rounded())
        if added > 0, gained == 0 { gained = 1 }
        if removed > 0, gained == lit, lit > 1 { gained = lit - 1 }
        return (gained, lit - gained)
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
                KouEmpty(symbol: "doc.badge.gearshape", title: "Binary file", message: "There is no text diff to show.")
            } else if let error {
                KouEmpty(symbol: "exclamationmark.triangle", title: "Diff unavailable", message: error, tone: .kouErr)
            } else {
                LoadingSpinner()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.kouGround)
        .safeAreaBar(edge: .top) {
            if text != nil || diff?.truncated == true {
                head
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
                .tint(Color.kouFg)
            }
        }
        .task { await load() }
    }

    /// As the cockpit heads a diff: the file's state, its counts — and, in
    /// the warning's colour, that the diff is cut short.
    private var head: some View {
        HStack(spacing: 10) {
            Text(verbatim: FileChangeRow.letter(file.status))
                .font(.system(size: 11, weight: .heavy, design: .monospaced))
                .foregroundStyle(FileChangeRow.tone(file.status))
            if file.binary != true {
                DiffCount(added: file.added ?? 0, removed: file.removed ?? 0)
                DiffMeter(added: file.added ?? 0, removed: file.removed ?? 0)
            }
            if diff?.truncated == true {
                Label("Cut short: too long to show whole", systemImage: "scissors")
                    .labelStyle(.titleAndIcon)
                    .foregroundStyle(Color.kouWarn)
                    .lineLimit(1)
            }
        }
        .font(.kouMeta)
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .glassEffect(.regular, in: .cornerCapsule)
        .padding(.top, 4)
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
                Self.head(change, failed: failed)
                    .font(.system(.caption2, design: .monospaced, weight: .semibold))
                    .textCase(.uppercase)
                    .tracking(0.6)
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
                    .foregroundStyle(Color.kouFg4)
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
                Button {
                    if let all { all() } else { withAnimation(.snappy) { open = true } }
                } label: {
                    HStack(spacing: 6) {
                        Text(verbatim: "Show \(shown.hidden) more \(shown.hidden == 1 ? "line" : "lines")")
                        Image(systemName: all == nil ? "chevron.down" : "arrow.up.left.and.arrow.down.right")
                            .font(.system(size: 9, weight: .bold))
                    }
                    .font(.kouEyebrow)
                    .textCase(.uppercase)
                    .tracking(Font.eyebrowTracking)
                    .foregroundStyle(Color.kouFg2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 9)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .overlay(alignment: .top) {
                    Rectangle()
                        .fill(Color.kouLine)
                        .frame(height: 1)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.kouSurface)
        .clipShape(.rect(cornerRadius: .corner(10)))
        .overlay(RoundedRectangle(cornerRadius: .corner(10)).strokeBorder(Color.kouLine, lineWidth: 1))
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
        .foregroundStyle(copied ? Color.kouOK : Color.kouFg3)
        .accessibilityLabel(copied ? "Copied" : change.kind == "write" ? "Copy the content" : "Copy the diff")
        .sensoryFeedback(.success, trigger: copies)
        .task(id: copies) {
            guard copies > 0 else { return }
            try? await Task.sleep(for: .seconds(1.4))
            guard !Task.isCancelled else { return }
            withAnimation(.snappy) { copied = false }
        }
    }

    /// The head of a change, as the page's: what it is in its grey — in the
    /// failure's colour when it was not done —, its counts in theirs.
    static func head(_ change: Tool.Change, failed: Bool) -> Text {
        let edit = change.kind != "write"
        let word = Text(verbatim: edit ? (failed ? "Not applied" : "Diff") : (failed ? "Not written" : "Content"))
            .foregroundStyle(failed ? Color.kouErr : Color.kouFg3)
        let dot = Text(verbatim: " · ").foregroundStyle(Color.kouFg4)
        guard edit else {
            let n = change.lineCount
            return Text("\(word)\(dot)\(Text(verbatim: "\(n) \(n == 1 ? "line" : "lines")").foregroundStyle(Color.kouFg3))")
        }
        let counts = change.counts
        let numbers = Text("\(Text(verbatim: "+\(counts.added)").foregroundStyle(Color.kouOK)) \(Text(verbatim: "−\(counts.removed)").foregroundStyle(Color.kouErr))")
        if let n = change.replacements, n > 1 {
            return Text("\(word)\(dot)\(numbers)\(dot)\(Text(verbatim: "\(n) replacements").foregroundStyle(Color.kouFg3))")
        }
        return Text("\(word)\(dot)\(numbers)")
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
            DiffCount(added: counts.added, removed: counts.removed)
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
                    KouEmpty(symbol: "doc", title: "An empty file")
                } else {
                    LoadingSpinner()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color.kouGround)
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .navigationSubtitle(change.map { ChangeBlock.label($0, failed: failed) } ?? "")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
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
                    .tint(Color.kouFg)
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
            bold = ["gs", "gh", "gu", "cs"].contains(name)
        }

        func apply(to piece: inout AttributedString) {
            if let color { piece.foregroundColor = color }
            if bold { piece.inlinePresentationIntent = .stronglyEmphasized }
        }
    }
}

/// ChangeRowView is a row of a change, as the cockpit draws a diff's: a
/// line's numbers in the faintest grey, its sign in a diff and its code —
/// the lines put in on the success's tint, those taken out on the
/// failure's — or the lines between two hunks left out.
private struct ChangeRowView: View {
    let row: ChangeRows.Row
    let diff: Bool
    let digits: Int

    var body: some View {
        if row.kind == .skipped {
            Text(row.code)
                .foregroundStyle(Color.kouFg3)
                .padding(.horizontal, 10)
                .padding(.vertical, 2)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.kouFg.opacity(0.05))
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
                        .foregroundStyle(signTone)
                        .padding(.trailing, 6)
                        .accessibilityHidden(row.kind == .kept)
                }
                Text(row.code)
                    .foregroundStyle(row.kind == .kept ? Color.kouFg2 : Color.kouFg)
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
            .foregroundStyle(Color.kouFg4)
    }

    private var sign: String {
        switch row.kind {
        case .added: "+"
        case .removed: "-"
        default: " "
        }
    }

    private var signTone: Color {
        switch row.kind {
        case .added: .kouOK
        case .removed: .kouErr
        default: .kouFg4
        }
    }

    private var tint: Color {
        switch row.kind {
        case .added: Color.kouOK.opacity(0.11)
        case .removed: Color.kouErr.opacity(0.10)
        default: .clear
        }
    }
}
