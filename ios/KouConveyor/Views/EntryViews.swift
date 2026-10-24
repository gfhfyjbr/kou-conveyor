import SwiftUI
import UIKit

/// What the entries of a session's transcript can do in it: show a prompt's
/// images and the pictures the agent looked at, branch the session before a
/// prompt, show what a prompt changed.
struct EntryContext {
    /// The path of a prompt's image, by its place among the prompt's.
    var imagePath: (Entry, Int) -> String?
    /// The path of the picture a ViewImage call read.
    var toolImagePath: (Entry) -> String?
    let client: RelayClient
    var branch: (Entry) -> Void
    var changes: (Entry) -> Void
    /// Shows a picture large over the screen, dimmed behind it, with what it
    /// is: "816×900 PNG · 14 KB".
    var picture: (_ path: String, _ title: String, _ about: String?) -> Void
    /// Edits a prompt: the session goes back to before it and runs the
    /// edited one. Nil while a prompt cannot be edited: something runs.
    var edit: ((Entry) -> Void)? = nil
    /// Shows all of what a Write call wrote, or an Edit call changed, apart:
    /// the transcript shows its start.
    var change: ((Entry) -> Void)? = nil
    /// The session's surfaces: a UI call is drawn as its surface.
    var surfaces: SurfaceBook? = nil
}

/// EntryView shows one block of a transcript, as the cockpit's timeline
/// does: the user's prompts, the agent's answers and thinking, its tools.
///
/// It is Equatable by what it shows: the screen around it draws itself anew
/// at every key pressed in the composer and every word the agent writes,
/// and the entries that did not change need not.
struct EntryView: View, @MainActor Equatable {
    let entry: Entry
    /// Whether a run goes on: an unfinished tool is at work, not cut short.
    let live: Bool
    /// What the command of a tool call that runs printed so far.
    var output: LiveOutput? = nil
    var context: EntryContext? = nil

    static func == (a: EntryView, b: EntryView) -> Bool {
        // The context's closures do the same from one drawing to the next;
        // what changes what is shown is whether a prompt can be edited.
        a.entry == b.entry && a.live == b.live && a.output == b.output
            && (a.context == nil) == (b.context == nil)
            && (a.context?.edit == nil) == (b.context?.edit == nil)
    }

    var body: some View {
        switch entry.kind {
        case Entry.user: UserBubble(entry: entry, context: context)
        case Entry.assistant: AssistantBlock(entry: entry)
        case Entry.reasoning: ReasoningBlock(entry: entry)
        case Entry.tool:
            if let context, let book = context.surfaces, entry.tool?.state == "done",
               let call = SurfaceCall(tool: entry.tool) {
                SurfaceBlock(entry: entry, call: call, book: book, context: context)
            } else {
                ToolBlock(entry: entry, live: live, output: output, context: context)
            }
        case Entry.error: ErrorBlock(text: entry.text ?? "", detail: entry.detail)
        default: NoticeRow(text: entry.text ?? "", detail: entry.detail)
        }
    }
}

struct UserBubble: View {
    let entry: Entry
    var context: EntryContext? = nil
    /// How wide the prompt is drawn: its copy, pressed, wraps as it does.
    @State private var width: CGFloat?

    /// The prompt's shape: round, but for the corner it is said from.
    static var shape: UnevenRoundedRectangle {
        UnevenRoundedRectangle(topLeadingRadius: .corner(18), bottomLeadingRadius: .corner(18),
                               bottomTrailingRadius: .corner(5), topTrailingRadius: .corner(18), style: .continuous)
    }

    /// The prompt, in the accent laid over the ground: the user's words
    /// stand out from the agent's by it, without a block of colour each.
    private var bubble: some View {
        Text(entry.text ?? "")
            .font(.body)
            .foregroundStyle(Color.kouFg)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background {
                ZStack {
                    Self.shape.fill(Color.kouGround)
                    Self.shape.fill(Color.kouAccent.opacity(0.14))
                    Self.shape.strokeBorder(Color.kouAccent.opacity(0.38), lineWidth: 1)
                }
            }
    }

    /// The prompt's images, with where the session keeps each.
    private var pictures: [(n: Int, label: String, path: String)] {
        guard let context, let images = entry.images else { return [] }
        return images.enumerated().compactMap { n, image in
            context.imagePath(entry, n).map { (n, image.label ?? "Image \(n + 1)", $0) }
        }
    }

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            if let context, !pictures.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(pictures, id: \.n) { picture in
                            Button {
                                context.picture(picture.path, picture.label,
                                                entry.images.flatMap { ViewedPicture.describe($0[picture.n]) })
                            } label: {
                                RemoteImage(client: context.client, path: picture.path)
                                    .frame(width: 72, height: 72)
                                    .clipShape(.rect(cornerRadius: .corner(14)))
                                    .overlay(RoundedRectangle(cornerRadius: .corner(14)).strokeBorder(Color.kouAccent.opacity(0.38), lineWidth: 1))
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel(picture.label)
                        }
                    }
                }
                .scrollClipDisabled()
                .frame(maxWidth: 260, alignment: .trailing)
                .fixedSize(horizontal: pictures.count < 4, vertical: false)
            }
            bubble
                // Pressed, the prompt lifts as it is drawn, not as a box.
                .contentShape(.contextMenuPreview, Self.shape)
                .contextMenu {
                    Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = entry.text }
                    if let context, entry.messageID != nil, entry.state == nil {
                        Button("Edit", systemImage: "pencil") { context.edit?(entry) }
                            .disabled(context.edit == nil)
                        Button("Branch from here", systemImage: "arrow.triangle.branch") { context.branch(entry) }
                        Button("Changes", systemImage: "plusminus.circle") { context.changes(entry) }
                    }
                } preview: {
                    // A copy, not the prompt in the transcript: that one
                    // moves while the session goes on, and a long one would
                    // not fit beside the menu.
                    bubble
                        .lineLimit(14)
                        .frame(width: width)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
                .opacity(entry.state == "pending" ? 0.7 : 1)
            // As the page's header of a prompt: when, with which model, and
            // how it went.
            HStack(spacing: 8) {
                if entry.forced == true {
                    Label("forced in", systemImage: "bolt.fill")
                        .foregroundStyle(Color.kouAccent)
                }
                switch entry.state {
                case "pending": Label("sending", systemImage: "clock")
                case "undelivered": Label("not delivered", systemImage: "exclamationmark.circle.fill").foregroundStyle(Color.kouErr)
                default: EmptyView()
                }
                if let images = entry.images, !images.isEmpty {
                    Label("\(images.count) image\(images.count == 1 ? "" : "s")", systemImage: "photo")
                }
                if let model = entry.model, !model.isEmpty {
                    Text(model)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                if let at = entry.at {
                    Text(Format.time(at))
                        .monospacedDigit()
                        .foregroundStyle(Color.kouFg3)
                }
            }
            .font(.system(.caption2, design: .monospaced))
            .foregroundStyle(Color.kouFg4)
            .labelStyle(.titleAndIcon)
            .padding(.trailing, 4)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.leading, 40)
    }
}

/// PromptEditor edits a prompt the session has: run, the session goes back
/// to before it and runs the edited prompt from there, as the cockpit's
/// Edit does. Files the agent changed keep their changes.
struct PromptEditor: View {
    /// How many entries come after the prompt: those the edit replaces.
    let after: Int
    /// Runs the edited prompt; it says whether the run started.
    var run: (String) async -> Bool
    @State private var text: String
    @State private var running = false
    @FocusState private var focused: Bool
    @Environment(\.dismiss) private var dismiss

    init(entry: Entry, after: Int, run: @escaping (String) async -> Bool) {
        self.after = after
        self.run = run
        _text = State(initialValue: entry.text ?? "")
    }

    private var empty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 10) {
                TextEditor(text: $text)
                    .focused($focused)
                    .scrollContentBackground(.hidden)
                    .padding(10)
                    .kouPanel(radius: 16, stroke: Color.kouAccent.opacity(0.38))
                if after > 0 {
                    Text("Replaces the \(after == 1 ? "entry" : "\(after) entries") below · changes to files stay")
                        .font(.footnote)
                        .foregroundStyle(Color.kouFg3)
                }
            }
            .padding()
            .background(Color.kouGround)
            .navigationTitle("Edit prompt")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", role: .cancel) { dismiss() }
                        .tint(Color.kouFg)
                }
                ToolbarItem(placement: .confirmationAction) {
                    if running {
                        ProgressView()
                    } else {
                        Button("Run", systemImage: "arrow.up") {
                            running = true
                            Task {
                                if await run(text) { dismiss() } else { running = false }
                            }
                        }
                        .disabled(empty)
                    }
                }
            }
        }
        .onAppear { focused = true }
        .interactiveDismissDisabled(running)
    }
}

struct AssistantBlock: View {
    let entry: Entry
    @Environment(\.streamMotion) private var motion
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        // As it streams, what the agent writes comes in at the end of the
        // text, after it a caret.
        let streaming = entry.streaming == true
        let revealing = streaming && motion && !reduceMotion
        MarkdownText(text: entry.text ?? "", streaming: streaming)
            .environment(\.textReveal, revealing)
            .environment(\.textCaret, revealing)
            .opacity(entry.phase == "commentary" ? 0.82 : 1)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contextMenu {
                Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = entry.text }
            } preview: {
                // Not the answer itself: a long one, lifted whole, is slow to
                // draw and too tall to read beside the menu.
                Text(entry.text ?? "")
                    .font(.subheadline)
                    .lineLimit(16)
                    .padding(14)
                    .frame(width: 340, alignment: .leading)
            }
    }
}

struct ReasoningBlock: View {
    let entry: Entry
    @State private var open = false
    @Environment(\.streamMotion) private var motion
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let streaming = entry.streaming == true
        // While the agent thinks, the row says so: the brain beats, a light
        // runs over "Thinking", and the gist follows what it thinks about.
        let moving = streaming && motion && !reduceMotion
        let gist = Self.gist(of: entry.text ?? "", streaming: streaming)
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.snappy) { open.toggle() }
            } label: {
                // As the page: "Thinking", and the gist — the thought's first
                // line — so that a row of thoughts says what each was.
                HStack(spacing: 7) {
                    Image(systemName: "brain")
                        .font(.caption)
                        .foregroundStyle(moving ? Color.kouAccent : Color.kouFg3)
                        .symbolEffect(.pulse, isActive: moving)
                    Text("Thinking")
                        .textCase(.uppercase)
                        .font(.kouEyebrow)
                        .tracking(Font.eyebrowTracking)
                        .foregroundStyle(Color.kouFg3)
                        .modifier(Shimmer(active: moving))
                        .fixedSize()
                    if !open, let gist {
                        // A new line of thought comes up from under the last.
                        ZStack(alignment: .leading) {
                            Text(gist.text)
                                .font(.footnote)
                                .foregroundStyle(Color.kouFg3)
                                .lineLimit(1)
                                .id(gist.line)
                                .transition(.push(from: .bottom))
                        }
                        .clipped()
                        .animation(moving ? .smooth(duration: 0.35) : nil, value: gist.line)
                    }
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(Color.kouFg4)
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .font(.footnote.weight(.medium))
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .disabled((entry.text ?? "").isEmpty)
            if open {
                MarkdownText(text: entry.text ?? "", streaming: streaming)
                    .environment(\.textReveal, moving)
                    .environment(\.textCaret, moving)
                    .font(.footnote)
                    .foregroundStyle(Color.kouFg2)
                    .padding(.leading, 13)
                    .overlay(alignment: .leading) {
                        Rectangle()
                            .stroke(Color.kouLine2, style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                            .frame(width: 1)
                    }
                    .padding(.leading, 6)
                    .transition(.opacity)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .modifier(RiseIn(active: moving))
    }

    /// The gist of a thought, and the number of its line: the first line;
    /// while the agent thinks, as on the page, its latest title — a line
    /// all bold, as models head what they think about — or its last line.
    /// Its emphasis and heading marks are dropped.
    static func gist(of text: String, streaming: Bool) -> (line: Int, text: String)? {
        let lines = text.split(omittingEmptySubsequences: false, whereSeparator: \.isNewline)
        var found: (line: Int, text: String)?
        if streaming {
            if let n = lines.indices.last(where: { title(lines[$0]) != nil }) {
                found = (n, title(lines[n]) ?? "")
            } else if let n = lines.indices.last(where: { !lines[$0].trimmingCharacters(in: .whitespaces).isEmpty }) {
                found = (n, lines[n].trimmingCharacters(in: .whitespaces))
            }
        } else if let n = lines.indices.first(where: { !lines[$0].trimmingCharacters(in: .whitespaces).isEmpty }) {
            found = (n, lines[n].trimmingCharacters(in: .whitespaces))
        }
        guard let found else { return nil }
        let gist = found.text.replacingOccurrences(of: "**", with: "")
            .trimmingCharacters(in: CharacterSet(charactersIn: "# "))
        return gist.isEmpty ? nil : (found.line, gist)
    }

    /// The title a line is, all bold: "**Planning the change**", a colon
    /// after it or not.
    private static func title(_ line: Substring) -> String? {
        var line = line.trimmingCharacters(in: .whitespaces)
        if line.hasSuffix(":") { line.removeLast() }
        guard line.count > 4, line.hasPrefix("**"), line.hasSuffix("**") else { return nil }
        let title = line.dropFirst(2).dropLast(2)
        return title.isEmpty ? nil : String(title)
    }
}

struct ToolBlock: View {
    let entry: Entry
    let live: Bool
    /// What the call's command printed so far, while it runs.
    var output: LiveOutput? = nil
    /// Where the picture a ViewImage call read comes from, and goes to
    /// when tapped.
    var context: EntryContext? = nil
    @State private var open = false
    @Environment(\.streamMotion) private var motion
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// The classes of the colours of what a Write or an Edit call wrote.
    @Environment(\.syntaxClasses) private var syntaxClasses

    var body: some View {
        let tool = entry.tool
        let state = ToolBlock.state(tool, live: live)
        // What the command printed shows as it prints it, until the call
        // is done and has its output.
        let running = state == "running" || state == "queued" ? output : nil
        VStack(alignment: .leading, spacing: 10) {
            Button {
                withAnimation(.snappy) { open.toggle() }
            } label: {
                // As the page's head of a call: its state's glyph, its name
                // as a label, and what it was given, which says the most.
                HStack(spacing: 8) {
                    ToolStateIcon(tool: tool, live: live)
                        .frame(width: 18)
                    Text(tool?.name ?? "Tool")
                        .textCase(.uppercase)
                        .font(.kouEyebrow)
                        .tracking(0.9)
                        .foregroundStyle(state == "failed" ? Color.kouErr : Color.kouFg2)
                        .lineLimit(1)
                        .layoutPriority(1)
                    summary(tool)
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundStyle(Color.kouFg)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 0)
                    meta(tool, state: state)
                        .layoutPriority(1)
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(Color.kouFg4)
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .contextMenu {
                if let input = tool?.input, !input.isEmpty {
                    Button(tool?.name?.lowercased() == "bash" ? "Copy command" : "Copy input", systemImage: "doc.on.doc") {
                        UIPasteboard.general.string = input
                    }
                }
                if let output = tool?.output, !output.isEmpty {
                    Button("Copy output", systemImage: "doc.on.doc") { UIPasteboard.general.string = output }
                }
                if let change = tool?.change, let text = change.text, !text.isEmpty {
                    Button(change.kind == "write" ? "Copy content" : "Copy diff", systemImage: "doc.on.doc") {
                        UIPasteboard.general.string = text
                    }
                }
            }

            // As the page: the picture the call read, under it, small while
            // the call is folded and larger once it is open.
            if let image = tool?.image, let context, let path = context.toolImagePath(entry) {
                ViewedPicture(image: image, path: path, context: context, large: open)
                    .padding(.leading, 26)
            }

            if open, let tool {
                details(tool, state: state, running: running)
                    .transition(.opacity)
            } else if state == "failed", let error = tool?.error, !error.isEmpty {
                // Closed, a failed call still says why.
                Text(ToolSummary.firstLine(error))
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(Color.kouErr)
                    .lineLimit(2)
                    .padding(.leading, 26)
            } else if let running, !running.all.isEmpty {
                // Closed, a call that runs shows the last lines its command
                // printed, as the page does; tapped, it opens.
                LiveTail(output: running, moving: motion && !reduceMotion)
                    .padding(.leading, 26)
                    .contentShape(.rect)
                    .onTapGesture { withAnimation(.snappy) { open = true } }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 11)
        .frame(maxWidth: .infinity, alignment: .leading)
        // A failed call's panel is edged in the failure's colour; a running
        // one has the run's light going round it.
        .kouPanel(radius: 12, stroke: state == "failed" ? Color.kouErr.opacity(0.4) : Color.kouLine)
        .runLight(state == "running", in: RoundedRectangle(cornerRadius: .corner(12), style: .continuous))
        .animation(.smooth(duration: 0.3), value: state)
    }

    /// The gist of the call's input; a command, as the page writes it, after
    /// a prompt's "$".
    private func summary(_ tool: Tool?) -> Text {
        let gist = ToolSummary.of(tool)
        guard tool?.name?.lowercased() == "bash", !gist.isEmpty else { return Text(gist) }
        return Text("\(Text("$ ").foregroundStyle(Color.kouFg4))\(gist)")
    }

    /// What the page writes after a call's input: what a Write or an Edit
    /// call changed, how long it ran, its exit code when not 0, and its state
    /// when it did not simply finish.
    @ViewBuilder
    private func meta(_ tool: Tool?, state: String) -> some View {
        HStack(spacing: 6) {
            if let change = tool?.change {
                ChangeStat(change: change)
            }
            if state == "running", let started = tool?.started {
                Text(timerInterval: started...Date.distantFuture, countsDown: false)
                    .monospacedDigit()
            } else if let started = tool?.started, let finished = tool?.finished {
                Text(Format.duration(finished.timeIntervalSince(started)))
                    .monospacedDigit()
            }
            if let code = tool?.exitCode, code != 0 {
                Text("Exit \(code)")
                    .foregroundStyle(Color.kouWarn)
            }
            if let word = ToolBlock.word[state] {
                Text(word)
                    .textCase(.uppercase)
                    .fontWeight(.bold)
                    .tracking(0.6)
                    .foregroundStyle(state == "failed" ? Color.kouErr : Color.kouFg3)
            }
        }
        .font(.system(.caption2, design: .monospaced))
        .foregroundStyle(Color.kouFg4)
        .lineLimit(1)
        .fixedSize()
    }

    /// The open call, as the page has it: the input when the line above
    /// does not already say it all, the calls its code made, why it failed,
    /// and what it wrote — as it writes it, while it runs.
    @ViewBuilder
    private func details(_ tool: Tool, state: String, running: LiveOutput?) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            if let input = tool.input, input.contains("\n") || input.count > 90 {
                CodeBlock(language: tool.name?.lowercased() == "code" ? "code" : "input", code: ToolSummary.pretty(input), maxLines: 30, keep: .head)
            }
            if let calls = tool.calls, !calls.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    SectionLabel("Calls", count: calls.count)
                    ForEach(Array(calls.enumerated()), id: \.offset) { _, call in
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Image(systemName: ToolBlock.callSymbol(call.state))
                                .foregroundStyle(call.state == "failed" ? Color.kouErr : Color.kouFg4)
                            Text(call.name).bold()
                                .foregroundStyle(Color.kouFg)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(call.gist ?? call.input ?? "").foregroundStyle(Color.kouFg3).lineLimit(1)
                                if let error = call.error, !error.isEmpty {
                                    Text(ToolSummary.firstLine(error)).foregroundStyle(Color.kouErr).lineLimit(2)
                                }
                            }
                        }
                        .font(.system(.caption, design: .monospaced))
                    }
                }
            }
            if let error = tool.error, !error.isEmpty {
                Text(error)
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(Color.kouErr)
                    .textSelection(.enabled)
            }
            if let change = tool.change, change.text != nil {
                // What the call wrote or changed, and under it what it said
                // it did: the lines around the change after it, its output
                // goes on with, the diff shows.
                ChangeBlock(change: change, failed: state == "failed", classes: syntaxClasses, all: showChange)
                    .equatable()
                if let output = tool.output, !output.isEmpty {
                    Text(ChangeBlock.said(output))
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(Color.kouFg3)
                        .textSelection(.enabled)
                }
            } else if let output = tool.output, !output.isEmpty {
                CodeBlock(language: "output", code: ToolSummary.tail(output), maxLines: 60)
            } else if let running, !running.out.isEmpty {
                CodeBlock(language: "output", code: ToolSummary.tail(Terminal.fold(running.out)), maxLines: 60)
            }
            if let stderr = tool.stderr, !stderr.isEmpty {
                CodeBlock(language: "stderr", code: ToolSummary.tail(stderr), maxLines: 30)
            } else if let running, !running.err.isEmpty {
                CodeBlock(language: "stderr", code: ToolSummary.tail(Terminal.fold(running.err)), maxLines: 30)
            }
            if let running, running.skipped > 0 || running.cut {
                Text(Self.skippedNote(running))
                    .font(.caption2)
                    .foregroundStyle(Color.kouFg4)
            }
            if let logs = tool.logs, !logs.isEmpty {
                CodeBlock(language: "console", code: ToolSummary.tail(logs), maxLines: 30)
            }
            if let value = tool.value, !value.isEmpty {
                CodeBlock(language: "return value", code: ToolSummary.tail(ToolSummary.pretty(value)), maxLines: 30)
            }
            if let files = tool.files, !files.isEmpty {
                Label(files.joined(separator: ", "), systemImage: "doc")
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(Color.kouFg3)
            }
            if [tool.output, tool.stderr, tool.error, tool.logs, tool.value].allSatisfy({ ($0 ?? "").isEmpty }), (tool.calls ?? []).isEmpty,
               (running?.all ?? "").isEmpty, tool.change?.text == nil
            {
                Eyebrow(state == "running" || state == "queued" ? "Waiting for output…" : "No output", color: .kouFg4)
            }
        }
    }

    /// Shows all of what the call wrote or changed, apart; nil where the
    /// transcript cannot.
    private var showChange: (() -> Void)? {
        guard let show = context?.change else { return nil }
        let entry = entry
        return { show(entry) }
    }

    /// What of a running command's output does not show, as the page says it.
    static func skippedNote(_ live: LiveOutput) -> String {
        var notes: [String] = []
        if live.skipped > 0 { notes.append("\(Format.bytes(live.skipped)) it printed faster than it could be shown were skipped") }
        if live.cut { notes.append("only the end of the output shows until the command is done") }
        let note = notes.joined(separator: "; ")
        return note.prefix(1).uppercased() + note.dropFirst()
    }

    /// A call's state as the page tells it: one that never finished in a
    /// session nobody runs was interrupted.
    static func state(_ tool: Tool?, live: Bool) -> String {
        let state = tool?.state ?? "queued"
        if !live, state == "queued" || state == "running" || state.isEmpty { return "interrupted" }
        return state
    }

    static let word = ["queued": "Queued", "failed": "Failed", "canceled": "Stopped", "interrupted": "Interrupted"]

    static func callSymbol(_ state: String) -> String {
        switch state {
        case "failed": "xmark.circle"
        case "running": "circle.dashed"
        case "queued": "circle.dotted"
        default: "arrow.turn.down.right"
        }
    }
}

/// ViewedPicture is the picture a ViewImage call read, as the model got it:
/// a thumbnail that opens it large over the dimmed screen, and what it is.
private struct ViewedPicture: View {
    let image: Entry.ImageInfo
    let path: String
    let context: EntryContext
    /// Whether the call is open: the picture is larger then.
    let large: Bool

    var body: some View {
        let fitted = Self.fit(image, within: large ? CGSize(width: 280, height: 440) : CGSize(width: 260, height: 200))
        // Too thin a picture to tap gets a frame big enough, around it.
        let frame = CGSize(width: max(fitted.width, 44), height: max(fitted.height, 44))
        VStack(alignment: .leading, spacing: 6) {
            Button {
                context.picture(path, title, Self.describe(image))
            } label: {
                RemoteImage(client: context.client, path: path, contentMode: .fit, retryMissing: true)
                    .frame(width: frame.width, height: frame.height)
                    .background(Color.kouSurface)
                    .clipShape(.rect(cornerRadius: .corner(10)))
                    .overlay(RoundedRectangle(cornerRadius: .corner(10)).strokeBorder(Color.kouLine2, lineWidth: 1))
                    .contentShape(.rect(cornerRadius: .corner(10)))
            }
            .buttonStyle(PressStyle())
            .accessibilityLabel("Picture \(title)")
            .accessibilityHint("Shows it large")
            if let about = Self.describe(image) {
                Text(about)
                    .font(.system(.caption2, design: .monospaced))
                    .foregroundStyle(Color.kouFg4)
                    .lineLimit(1)
            }
        }
    }

    private var title: String {
        let label = image.label?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return label.isEmpty ? "Picture" : label
    }

    /// "920×2000 PNG · 307 KB", as the page writes it.
    static func describe(_ image: Entry.ImageInfo) -> String? {
        var kind = [String]()
        if let width = image.width, let height = image.height, width > 0, height > 0 { kind.append("\(width)×\(height)") }
        if let type = image.mediaType, !type.isEmpty { kind.append(type.replacingOccurrences(of: "image/", with: "").uppercased()) }
        let parts = [kind.joined(separator: " "), image.size.map { Format.bytes(Int64($0)) } ?? ""].filter { !$0.isEmpty }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// fit is the size a picture is shown at: whole within the box, and
    /// never larger than it is.
    static func fit(_ image: Entry.ImageInfo, within box: CGSize) -> CGSize {
        guard let w = image.width, let h = image.height, w > 0, h > 0 else { return CGSize(width: 160, height: 120) }
        let width = CGFloat(w), height = CGFloat(h)
        let scale = min(box.width / width, box.height / height, 1)
        return CGSize(width: (width * scale).rounded(), height: (height * scale).rounded())
    }
}

/// PressStyle sinks a thumbnail a little while it is pressed.
private struct PressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .opacity(configuration.isPressed ? 0.85 : 1)
            .animation(.snappy(duration: 0.18), value: configuration.isPressed)
    }
}

struct ToolStateIcon: View {
    let tool: Tool?
    let live: Bool

    var body: some View {
        // The page's glyphs — □ ■ ✓ ✕ ⊘ ◌ — light, so that a column of
        // finished calls stays quiet and the running and failed ones show.
        Group {
            switch ToolBlock.state(tool, live: live) {
            case "running":
                LiveDot(size: 6)
            case "queued":
                Image(systemName: "square")
                    .imageScale(.small)
                    .foregroundStyle(Color.kouFg3)
            case "done":
                Image(systemName: "checkmark")
                    .foregroundStyle(tool?.exitCode.map { $0 != 0 } == true ? Color.kouWarn : Color.kouOK)
            case "failed":
                Image(systemName: "xmark").foregroundStyle(Color.kouErr)
            case "canceled":
                Image(systemName: "nosign").foregroundStyle(Color.kouFg3)
            default:
                // Unfinished, with no run: cut short.
                Image(systemName: "circle.dashed").foregroundStyle(Color.kouFg3)
            }
        }
        .font(.footnote.weight(.bold))
        .contentTransition(.symbolEffect(.replace))
    }
}

enum ToolSummary {
    /// The gist of a tool's input: its command, its file, or its first line.
    static func of(_ tool: Tool?) -> String {
        guard let input = tool?.input?.trimmingCharacters(in: .whitespacesAndNewlines), !input.isEmpty else { return "" }
        if input.hasPrefix("{"), let data = input.data(using: .utf8),
           let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        {
            for key in ["command", "cmd", "path", "file_path", "pattern", "query", "url", "name", "prompt"] {
                if let value = object[key] as? String, !value.isEmpty { return firstLine(value) }
                if let value = object[key] as? [String], !value.isEmpty { return firstLine(value.joined(separator: " ")) }
            }
        }
        return firstLine(input)
    }

    static func firstLine(_ text: String) -> String {
        String(text.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: true).first ?? "").prefix(160).description
    }

    /// JSON input, indented.
    static func pretty(_ input: String) -> String {
        guard input.hasPrefix("{") || input.hasPrefix("["), let data = input.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data),
              let pretty = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]),
              let text = String(data: pretty, encoding: .utf8)
        else { return input }
        return text
    }

    /// The end of a long output, which is what matters most.
    static func tail(_ text: String, limit: Int = 6000) -> String {
        guard text.count > limit else { return text }
        return "…\n" + text.suffix(limit)
    }
}

struct NoticeRow: View {
    let text: String
    let detail: String?
    @State private var open = false

    var body: some View {
        VStack(spacing: 8) {
            Button {
                if expandable { withAnimation(.snappy) { open.toggle() } }
            } label: {
                // As a mark on a rule: the transcript goes on through it.
                HStack(spacing: 10) {
                    Hairline()
                        .frame(minWidth: 14)
                    HStack(alignment: .firstTextBaseline, spacing: 5) {
                        Text(text)
                            .multilineTextAlignment(.center)
                        if expandable {
                            // As the page's notice with a detail: it opens.
                            Image(systemName: "chevron.right")
                                .font(.caption2.weight(.bold))
                                .foregroundStyle(Color.kouFg4)
                                .rotationEffect(.degrees(open ? 90 : 0))
                        }
                    }
                    .font(.footnote)
                    .foregroundStyle(Color.kouFg3)
                    .fixedSize(horizontal: false, vertical: true)
                    .layoutPriority(1)
                    Hairline()
                        .frame(minWidth: 14)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .disabled(!expandable)
            .accessibilityLabel(text)
            if open, let detail {
                MarkdownText(text: detail)
                    .font(.footnote)
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .kouPanel(radius: 12)
                    .transition(.opacity)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 2)
    }

    private var expandable: Bool { detail?.isEmpty == false }
}

struct ErrorBlock: View {
    let text: String
    let detail: String?
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Eyebrow("Error", color: .kouErr)
            Text(text)
                .font(.subheadline.weight(.medium))
                .foregroundStyle(Color.kouFg)
                .textSelection(.enabled)
            if let detail, !detail.isEmpty {
                Text(detail)
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(Color.kouFg2)
                    .lineLimit(open ? nil : 12)
                    .textSelection(.enabled)
                if detail.split(separator: "\n", omittingEmptySubsequences: false).count > 12 || detail.count > 900 {
                    Button(open ? "Show less" : "Show all") {
                        withAnimation(.snappy) { open.toggle() }
                    }
                    .font(.caption.weight(.medium))
                    .buttonStyle(.borderless)
                    .tint(Color.kouErr)
                }
            }
        }
        .padding(.vertical, 12)
        .padding(.trailing, 12)
        .padding(.leading, 15)
        .frame(maxWidth: .infinity, alignment: .leading)
        .kouPanel(radius: 12, fill: Color.kouErr.opacity(0.07), stroke: Color.kouErr.opacity(0.35))
        // Its edge in the failure's colour, as the cockpit marks an error.
        .overlay(alignment: .leading) {
            UnevenRoundedRectangle(topLeadingRadius: .corner(12), bottomLeadingRadius: .corner(12), style: .continuous)
                .fill(Color.kouErr)
                .frame(width: 3)
        }
        .contextMenu {
            Button("Copy", systemImage: "doc.on.doc") {
                UIPasteboard.general.string = [text, detail ?? ""].filter { !$0.isEmpty }.joined(separator: "\n\n")
            }
        }
    }
}

/// CodeBlock is code, or a tool's output: monospaced, scrolled sideways.
/// A long one shows its end — or its start, for code — and says how much it
/// leaves out.
struct CodeBlock: View {
    enum Keep { case head, tail }

    let language: String?
    let code: String
    var maxLines: Int? = nil
    var keep: Keep = .tail
    @State private var copied = false
    /// Counts the copies: each one keeps the check a while longer.
    @State private var copies = 0
    /// Whether the code comes in as the agent writes it, the caret after it.
    @Environment(\.textReveal) private var reveal
    @Environment(\.textCaret) private var caret

    var body: some View {
        let shown = shown
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                if let language, !language.isEmpty {
                    Eyebrow(language, color: language == "stderr" ? .kouWarn : .kouFg4)
                }
                Spacer()
                Button {
                    UIPasteboard.general.string = code
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
                .accessibilityLabel(copied ? "Copied" : "Copy")
                .sensoryFeedback(.success, trigger: copies)
                .task(id: copies) {
                    guard copies > 0 else { return }
                    try? await Task.sleep(for: .seconds(1.4))
                    guard !Task.isCancelled else { return }
                    withAnimation(.snappy) { copied = false }
                }
            }
            .padding(.leading, 10)
            .padding(.trailing, 2)
            .padding(.top, 2)
            if shown.hidden > 0, keep == .tail {
                more(shown.hidden, "above")
            }
            ScrollView(.horizontal, showsIndicators: false) {
                Text(shown.text)
                    .font(.system(.footnote, design: .monospaced))
                    .fixedSize(horizontal: true, vertical: false)
                    // Selectable through Reveal. The length, counted only as
                    // the code streams, in UTF-16 units: the renderer makes
                    // up the difference with the glyphs.
                    .modifier(Reveal(length: reveal || caret ? shown.text.utf16.count : 0, reveal: reveal, caret: caret))
                    .padding(.horizontal, 10)
                    .padding(.top, shown.hidden > 0 && keep == .tail ? 2 : 0)
                    .padding(.bottom, 10)
            }
            .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
            if shown.hidden > 0, keep == .head {
                more(shown.hidden, "below")
                    .padding(.bottom, 8)
                    .padding(.top, -4)
            }
        }
        .kouPanel(radius: 10, fill: .kouSurface)
    }

    private func more(_ n: Int, _ where: String) -> some View {
        Text("… \(n) more line\(n == 1 ? "" : "s") \(`where`)")
            .font(.system(.caption2, design: .monospaced))
            .foregroundStyle(Color.kouFg4)
            .padding(.horizontal, 10)
    }

    /// The lines shown, and how many are not.
    private var shown: (text: String, hidden: Int) {
        let trimmed = code.hasSuffix("\n") ? String(code.dropLast()) : code
        guard let maxLines else { return (trimmed, 0) }
        let lines = trimmed.split(separator: "\n", omittingEmptySubsequences: false)
        guard lines.count > maxLines else { return (trimmed, 0) }
        let kept = keep == .tail ? lines.suffix(maxLines) : lines.prefix(maxLines)
        return (kept.joined(separator: "\n"), lines.count - maxLines)
    }
}

// MARK: What streams

/// StreamSettings is the user's say in how what streams of the agent's
/// answer and thinking shows, as the cockpit's Settings has it.
enum StreamSettings {
    /// Whether it moves as it comes: its words come in, a caret blinks after
    /// them, the thinking's label shimmers. On unless turned off.
    static let motionKey = "transcript.streamMotion"
}

extension EnvironmentValues {
    /// Whether what streams moves as it comes, as the user set it
    /// (StreamSettings); the system's Reduce Motion is apart.
    @Entry var streamMotion: Bool = true
    /// Whether the glyphs a text gains come in, rather than at once: those
    /// of a streaming preview, with the motion on.
    @Entry var textReveal: Bool = false
    /// Whether a caret ends a text: a streaming preview's, in its last block.
    @Entry var textCaret: Bool = false
}

/// Reveal draws a text as it grows: the glyphs it gains come in at its
/// end, out of a blur, and a caret follows them — solid while text comes,
/// blinking while it waits — as on the page. A text first shown while
/// nothing streams is drawn as any text.
struct Reveal: ViewModifier {
    /// How long the text is, in characters.
    let length: Int
    let reveal: Bool
    let caret: Bool
    /// Whether the text can be selected, once it is done: a selectable text
    /// is drawn past its renderer, so none is while it comes in. Reveal
    /// makes it selectable, not the text.
    let selectable: Bool
    /// Whether RevealRenderer draws the text: from when it streamed until
    /// it is done and all of it shows.
    @State private var engaged: Bool
    /// How much of it shows (RevealRenderer.shown).
    @State private var shown: Double
    /// When it last grew: the caret is solid a while after.
    @State private var grew = Date()
    /// Whether it still streams, as known last: an animation's completion
    /// would read reveal and caret as they were when it began.
    @State private var streams: Bool
    /// How many animations of what shows are under way.
    @State private var growing = 0

    /// How long the caret is on, then off, while it blinks.
    static let blink = 0.55
    /// How much of a text first shown as it streams comes in: its end. A
    /// thought opened, an answer scrolled back to, shows what came before.
    static let tail = 160

    init(length: Int, reveal: Bool, caret: Bool, selectable: Bool = true) {
        self.length = length
        self.reveal = reveal
        self.caret = caret
        self.selectable = selectable
        _engaged = State(initialValue: reveal || caret)
        _streams = State(initialValue: reveal || caret)
        _shown = State(initialValue: reveal ? Double(max(0, length - Self.tail)) : RevealRenderer.end(of: length))
    }

    func body(content: Content) -> some View {
        if engaged {
            // Each blink, while the caret blinks; otherwise all but never.
            TimelineView(.periodic(from: grew, by: caret && reveal ? Self.blink : 3600)) { timeline in
                content.textRenderer(RevealRenderer(shown: shown, target: RevealRenderer.end(of: length),
                                                    caret: caret ? caretShown(at: timeline.date) : 0))
            }
            .onChange(of: length, initial: true) { _, length in grow(to: length) }
            .onChange(of: reveal || caret) { _, streams in
                self.streams = streams
                settle()
            }
        } else if selectable {
            content.textSelection(.enabled)
        } else {
            content
        }
    }

    /// The caret: solid as the text grows; then off and on, a blink each.
    private func caretShown(at date: Date) -> Double {
        guard reveal else { return 1 }
        let blinks = Int((max(0, date.timeIntervalSince(grew)) / Self.blink + 0.01).rounded(.down))
        return blinks % 2 == 0 ? 1 : 0
    }

    private func grow(to length: Int) {
        let target = RevealRenderer.end(of: length)
        guard target != shown else { return }
        grew = Date()
        guard reveal, target > shown else {
            shown = target
            settle()
            return
        }
        // A piece comes in over a moment, a longer one over a longer one.
        let pace = min(0.8, max(0.35, (target - shown) / 500))
        growing += 1
        withAnimation(.smooth(duration: pace)) {
            shown = target
        } completion: {
            growing -= 1
            settle()
        }
    }

    /// settle lets the renderer go once the text is done and all of it
    /// shows: drawn as any text from then on.
    private func settle() {
        if engaged, !streams, growing <= 0 { engaged = false }
    }
}

/// RevealRenderer draws a text whose end is coming in: its glyphs up to
/// shown less the edge whole, the edge's coming in — fading in out of a
/// blur, rising a little into place — those after it not yet, and the
/// caret after the last shown.
struct RevealRenderer: TextRenderer {
    /// How many glyphs show, the last edge of them coming in.
    var shown: Double
    /// Where shown goes: once there, the whole text shows.
    let target: Double
    /// How much the caret shows: 0 for none.
    let caret: Double

    /// How many glyphs come in at once.
    static let edge = 10.0

    /// Where shown ends for a text of length characters: past it by the
    /// edge, so that its last glyphs come in whole.
    static func end(of length: Int) -> Double { Double(length) + edge }

    var animatableData: Double {
        get { shown }
        set { shown = newValue }
    }

    /// Room for the glyphs that rise into place, their blur and the caret.
    var displayPadding: EdgeInsets { EdgeInsets(top: 4, leading: 4, bottom: 8, trailing: 16) }

    func draw(layout: Text.Layout, in context: inout GraphicsContext) {
        let whole = shown >= target
        // shown counts characters and the layout glyphs, fewer of them —
        // a line's break has none — or more: the difference is the text's
        // end's, which is where it grows. front is shown in glyphs.
        let glyphs = layout.reduce(0) { $0 + $1.reduce(0) { $0 + $1.count } }
        let front = shown - (target - Self.edge - Double(glyphs))
        let done = front - Self.edge
        // The first glyph of the line, or of the run, drawn next.
        var start = 0.0
        // The bounds of the last glyph shown: the caret goes after it.
        var last: CGRect?
        lines: for line in layout {
            let count = Double(line.reduce(0) { $0 + $1.count })
            if whole || start + count <= done {
                context.draw(line)
                last = line.typographicBounds.rect
                start += count
                continue
            }
            for run in line {
                let n = run.count
                if start + Double(n) <= done {
                    context.draw(run)
                    last = run.typographicBounds.rect
                    start += Double(n)
                    continue
                }
                // Those of its glyphs that came in, at once; then those
                // coming in, one by one.
                let came = min(n, max(0, Int((done - start).rounded(.down)) + 1))
                if came > 0 {
                    let slice = run[0..<came]
                    context.draw(slice)
                    last = slice.typographicBounds.rect
                }
                for i in came..<n {
                    let progress = (front - start - Double(i)) / Self.edge
                    guard progress > 0 else { break lines }
                    let glyph = run[i]
                    draw(glyph, coming: min(1, progress), in: context)
                    last = glyph.typographicBounds.rect
                }
                start += Double(n)
            }
        }
        if caret > 0 { drawCaret(after: last ?? layout.first?.typographicBounds.rect, at: last == nil, in: context) }
    }

    /// A glyph coming in: it fades in out of a blur, rising into place,
    /// quickly at first and slowly as it lands.
    private func draw(_ glyph: Text.Layout.RunSlice, coming progress: Double, in context: GraphicsContext) {
        var context = context
        let eased = 1 - pow(1 - progress, 3)
        let height = glyph.typographicBounds.rect.height
        context.opacity *= eased
        context.translateBy(x: 0, y: (1 - eased) * height * 0.2)
        let blur = (1 - eased) * height * 0.12
        if blur > 0.2 { context.addFilter(.blur(radius: blur)) }
        context.draw(glyph, options: .disablesSubpixelQuantization)
    }

    /// The caret, in the accent: after the glyph whose bounds these are,
    /// or, with none shown yet, at the start of the first line.
    private func drawCaret(after bounds: CGRect?, at start: Bool, in context: GraphicsContext) {
        guard let bounds, bounds.height > 0 else { return }
        var context = context
        let height = (bounds.height * 0.82).rounded()
        let width = max(2, (bounds.height * 0.38).rounded())
        let rect = CGRect(x: (start ? bounds.minX : bounds.maxX + 1.5).rounded(),
                          y: (bounds.minY + (bounds.height - height) / 2).rounded(),
                          width: width, height: height)
        context.opacity *= caret
        context.fill(Path(roundedRect: rect, cornerRadius: min(2, width / 3)), with: .color(.kouAccent))
    }
}

/// RiseIn brings a block in that comes as the agent writes: it fades in,
/// rising into place, as the page's previews do. One there before, or
/// shown while nothing streams, is just there.
struct RiseIn: ViewModifier {
    @State private var risen: Bool

    init(active: Bool) {
        _risen = State(initialValue: !active)
    }

    func body(content: Content) -> some View {
        content
            .opacity(risen ? 1 : 0)
            .offset(y: risen ? 0 : 6)
            .onAppear {
                guard !risen else { return }
                withAnimation(.smooth(duration: 0.4)) { risen = true }
            }
    }
}

/// Shimmer runs a light over a label while what it names goes on, as the
/// page's thinking label shimmers.
struct Shimmer: ViewModifier {
    let active: Bool

    /// How long the light takes to cross the label.
    static let period = 1.8

    func body(content: Content) -> some View {
        content.overlay {
            if active {
                TimelineView(.animation(minimumInterval: 1 / 30)) { timeline in
                    // By the clock: drawn again, it goes on from where it was.
                    let phase = timeline.date.timeIntervalSinceReferenceDate
                        .truncatingRemainder(dividingBy: Self.period) / Self.period
                    GeometryReader { proxy in
                        let width = proxy.size.width
                        let band = max(28, width * 0.6)
                        LinearGradient(colors: [.clear, Color.kouFg, .clear], startPoint: .leading, endPoint: .trailing)
                            .frame(width: band)
                            .offset(x: -band + (width + band) * phase)
                    }
                }
                .mask { content }
                .allowsHitTesting(false)
                .accessibilityHidden(true)
            }
        }
    }
}

/// Terminal is what a terminal shows of what a command printed.
enum Terminal {
    /// Of a line written over — a progress bar's, after \r — what was
    /// written last, as the page shows it.
    static func fold(_ text: String) -> String {
        guard text.unicodeScalars.contains("\r") else { return text }
        return text.replacingOccurrences(of: "\r\n", with: "\n")
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { (line: Substring) -> Substring in
                guard line.unicodeScalars.contains("\r") else { return line }
                return line.split(separator: "\r", omittingEmptySubsequences: false).last(where: { !$0.isEmpty }) ?? ""
            }
            .joined(separator: "\n")
    }
}

/// LiveTail is the last lines the command of a running call printed, under
/// the call closed, as the page shows them: a new line comes up from below
/// as the others go up, those above fading out at the top.
struct LiveTail: View {
    let output: LiveOutput
    /// Whether the lines move as they come.
    let moving: Bool

    /// How many lines show, as the page's TAIL.
    static let count = 6

    struct Line: Identifiable {
        /// The line's number among all the command printed: it keeps it
        /// as it goes up.
        let id: Int
        let text: String
    }

    /// Lines that went up out of the tail at once, and when.
    struct Scroll {
        let at: Date
        let lines: Int
    }

    /// How long the lines take to go up by those that came.
    static let duration = 0.3

    /// The number of the first line shown, as last counted.
    @State private var top: Int?
    /// The scrolls under way. The lines are down by the rest of them all,
    /// together, as in a terminal: however fast lines come, they go up as
    /// one and none runs into another.
    @State private var scrolls: [Scroll] = []

    var body: some View {
        // The lines shown, and as many above them for the lines to go up from.
        let lines = Self.lines(of: output, keeping: Self.count * 2)
        let shown = min(lines.count, Self.count)
        let first = lines.count > shown ? lines[lines.count - shown].id : lines.first?.id ?? 0
        // Lines above those shown: the first fades out.
        let more = first > 0
        // The lines that went up since they were last counted (onChange)
        // start going up now.
        let pending = moving ? max(0, first - (top ?? first)) : 0
        TimelineView(.animation(paused: scrolls.isEmpty && pending == 0)) { timeline in
            let down = moving ? Self.down(scrolls, pending: pending, at: timeline.date) : 0
            let rows = Double(max(1, shown))
            TailLayout(count: Self.count, spacing: 1) {
                ForEach(lines) { line in
                    Text(line.text.isEmpty ? " " : line.text)
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityHidden(line.id < first)
                }
            }
            // The tail is as tall as the lines shown, a line its height
            // over them, and the space between two.
            .visualEffect { effect, proxy in
                effect.offset(y: down * (proxy.size.height + 1) / rows)
            }
            .transaction { $0.animation = nil }
            .clipped()
        }
        .font(.system(.caption, design: .monospaced))
        .foregroundStyle(Color.kouFg2)
        .mask {
            LinearGradient(stops: [.init(color: .black.opacity(more ? 0.1 : 1), location: 0),
                                   .init(color: .black, location: more ? 0.5 : 0)],
                           startPoint: .top, endPoint: .bottom)
        }
        .onChange(of: first, initial: true) { _, first in
            let now = Date()
            if moving, let top, first > top {
                scrolls.removeAll { now.timeIntervalSince($0.at) >= Self.duration }
                scrolls.append(Scroll(at: now, lines: min(Self.count, first - top)))
            }
            top = first
        }
        .task(id: scrolls.last?.at) {
            // The last scroll over, all are: the time stops.
            guard let at = scrolls.last?.at else { return }
            try? await Task.sleep(for: .seconds(max(0, Self.duration - Date().timeIntervalSince(at))))
            if !Task.isCancelled { scrolls.removeAll() }
        }
        .accessibilityElement(children: .combine)
    }

    /// How many lines down from their places the lines are at a time: the
    /// rest of each scroll, the pending lines all, as the lines kept above
    /// allow. A scroll goes fast at first, then slows down.
    static func down(_ scrolls: [Scroll], pending: Int, at date: Date) -> Double {
        var down = Double(pending)
        for scroll in scrolls {
            let rest = 1 - min(1, max(0, date.timeIntervalSince(scroll.at) / duration))
            down += Double(scroll.lines) * rest * rest * rest
        }
        return min(Double(count), down)
    }

    /// The last lines printed, numbered: the last is the one the command
    /// writes on, its number that of the newlines it printed.
    static func lines(of output: LiveOutput, keeping: Int = count) -> [Line] {
        // The end is enough: the last lines are in it.
        let window = 8 * 1024
        var text = output.all
        var cut = false
        if text.utf8.count > window {
            text = String(decoding: text.utf8.suffix(window), as: UTF8.self)
            cut = true
        }
        let split = Terminal.fold(text).split(separator: "\n", omittingEmptySubsequences: false)
        var lines = split.enumerated().map { n, line in
            Line(id: output.lines - (split.count - 1 - n), text: String(line))
        }
        // Cut, the first is a piece of a line.
        if cut, !lines.isEmpty { lines.removeFirst() }
        while let last = lines.last, last.text.allSatisfy(\.isWhitespace) { lines.removeLast() }
        return Array(lines.suffix(keeping))
    }
}

/// TailLayout lays lines out one above another from its bottom up, as tall
/// as the last `count` of them: those above stand out of it, at its top,
/// for the lines to go up from.
struct TailLayout: Layout {
    let count: Int
    let spacing: Double

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let shown = subviews.suffix(count)
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil }
        var size = CGSize(width: width ?? 0, height: spacing * Double(max(0, shown.count - 1)))
        for line in shown {
            let fits = line.sizeThatFits(ProposedViewSize(width: width, height: nil))
            size.height += fits.height
            if width == nil { size.width = max(size.width, fits.width) }
        }
        return size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var bottom = bounds.maxY
        for line in subviews.reversed() {
            let height = line.sizeThatFits(ProposedViewSize(width: bounds.width, height: nil)).height
            line.place(at: CGPoint(x: bounds.minX, y: bottom), anchor: .bottomLeading,
                       proposal: ProposedViewSize(width: bounds.width, height: height))
            bottom -= height + spacing
        }
    }
}
