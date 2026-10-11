import SwiftUI
import UIKit

/// What the entries of a session's transcript can do in it: show a prompt's
/// images, branch the session before a prompt, show what a prompt changed.
struct EntryContext {
    /// The path of a prompt's image, by its place among the prompt's.
    var imagePath: (Entry, Int) -> String?
    let client: RelayClient
    var branch: (Entry) -> Void
    var changes: (Entry) -> Void
    var picture: (_ path: String, _ title: String) -> Void
    /// Edits a prompt: the session goes back to before it and runs the
    /// edited one. Nil while a prompt cannot be edited: something runs.
    var edit: ((Entry) -> Void)? = nil
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
    var context: EntryContext? = nil

    static func == (a: EntryView, b: EntryView) -> Bool {
        // The context's closures do the same from one drawing to the next;
        // what changes what is shown is whether a prompt can be edited.
        a.entry == b.entry && a.live == b.live
            && (a.context == nil) == (b.context == nil)
            && (a.context?.edit == nil) == (b.context?.edit == nil)
    }

    var body: some View {
        switch entry.kind {
        case Entry.user: UserBubble(entry: entry, context: context)
        case Entry.assistant: AssistantBlock(entry: entry)
        case Entry.reasoning: ReasoningBlock(entry: entry)
        case Entry.tool: ToolBlock(entry: entry, live: live)
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

    private var bubble: some View {
        Text(entry.text ?? "")
            .font(.body)
            .foregroundStyle(.white)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background(Color.kouAccent.gradient, in: .rect(cornerRadius: 20))
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
                                context.picture(picture.path, picture.label)
                            } label: {
                                RemoteImage(client: context.client, path: picture.path)
                                    .frame(width: 72, height: 72)
                                    .clipShape(.rect(cornerRadius: 14))
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
                .contentShape(.contextMenuPreview, .rect(cornerRadius: 20))
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
            HStack(spacing: 6) {
                if entry.forced == true {
                    Label("forced in", systemImage: "bolt.fill")
                }
                switch entry.state {
                case "pending": Label("sending", systemImage: "clock")
                case "undelivered": Label("not delivered", systemImage: "exclamationmark.circle.fill").foregroundStyle(.red)
                default: EmptyView()
                }
                if let images = entry.images, !images.isEmpty {
                    Label("\(images.count) image\(images.count == 1 ? "" : "s")", systemImage: "photo")
                }
                if let model = entry.model, !model.isEmpty {
                    Text(model)
                        .font(.system(.caption2, design: .monospaced))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                if let at = entry.at {
                    Text(Format.time(at))
                        .monospacedDigit()
                }
            }
            .font(.caption2)
            .foregroundStyle(.secondary)
            .labelStyle(.titleAndIcon)
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
                    .background(.fill.quaternary, in: .rect(cornerRadius: 16))
                if after > 0 {
                    Text("Replaces the \(after == 1 ? "entry" : "\(after) entries") below · changes to files stay")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .padding()
            .navigationTitle("Edit prompt")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", role: .cancel) { dismiss() }
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

    var body: some View {
        MarkdownText(text: entry.text ?? "")
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

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.snappy) { open.toggle() }
            } label: {
                // As the page: "Thinking", and the gist — the thought's first
                // line — so that a row of thoughts says what each was.
                HStack(spacing: 6) {
                    Image(systemName: "brain")
                    Text("Thinking")
                    if !open, let gist {
                        Text(gist)
                            .fontWeight(.regular)
                            .foregroundStyle(.tertiary)
                            .lineLimit(1)
                    }
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.bold))
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .font(.footnote.weight(.medium))
                .foregroundStyle(.secondary)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .disabled((entry.text ?? "").isEmpty)
            if open {
                MarkdownText(text: entry.text ?? "")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .padding(.leading, 12)
                    .overlay(alignment: .leading) {
                        Rectangle()
                            .stroke(.tertiary, style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                            .frame(width: 1)
                    }
                    .padding(.leading, 7)
                    .transition(.opacity)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// The thought's first line, its emphasis and heading marks dropped.
    private var gist: String? {
        let text = entry.text ?? ""
        guard let line = text.split(whereSeparator: \.isNewline).lazy
            .map({ $0.trimmingCharacters(in: .whitespaces) })
            .first(where: { !$0.isEmpty })
        else { return nil }
        let gist = line.replacingOccurrences(of: "**", with: "")
            .trimmingCharacters(in: CharacterSet(charactersIn: "# "))
        return gist.isEmpty ? nil : gist
    }
}

struct ToolBlock: View {
    let entry: Entry
    let live: Bool
    @State private var open = false

    var body: some View {
        let tool = entry.tool
        let state = ToolBlock.state(tool, live: live)
        VStack(alignment: .leading, spacing: 10) {
            Button {
                withAnimation(.snappy) { open.toggle() }
            } label: {
                HStack(spacing: 8) {
                    ToolStateIcon(tool: tool, live: live)
                        .frame(width: 18)
                    Text(tool?.name ?? "Tool")
                        .font(.system(.subheadline, design: .monospaced, weight: .semibold))
                        .lineLimit(1)
                        .layoutPriority(1)
                    Text(ToolSummary.of(tool))
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 0)
                    meta(tool, state: state)
                        .layoutPriority(1)
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(.tertiary)
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
            }

            if open, let tool {
                details(tool, state: state)
                    .transition(.opacity)
            } else if state == "failed", let error = tool?.error, !error.isEmpty {
                // Closed, a failed call still says why.
                Text(ToolSummary.firstLine(error))
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.red)
                    .lineLimit(2)
                    .padding(.leading, 26)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 14))
    }

    /// What the page writes after a call's input: how long it ran, its exit
    /// code when not 0, and its state when it did not simply finish.
    @ViewBuilder
    private func meta(_ tool: Tool?, state: String) -> some View {
        HStack(spacing: 6) {
            if state == "running", let started = tool?.started {
                Text(timerInterval: started...Date.distantFuture, countsDown: false)
                    .monospacedDigit()
            } else if let started = tool?.started, let finished = tool?.finished {
                Text(Format.duration(finished.timeIntervalSince(started)))
                    .monospacedDigit()
            }
            if let code = tool?.exitCode, code != 0 {
                Text("Exit \(code)")
                    .foregroundStyle(.orange)
            }
            if let word = ToolBlock.word[state] {
                Text(word)
                    .textCase(.uppercase)
                    .fontWeight(.bold)
                    .foregroundStyle(state == "failed" ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
            }
        }
        .font(.system(.caption2, design: .monospaced))
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .fixedSize()
    }

    /// The open call, as the page has it: the input when the line above
    /// does not already say it all, the calls its code made, why it failed,
    /// and what it wrote.
    @ViewBuilder
    private func details(_ tool: Tool, state: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            if let input = tool.input, input.contains("\n") || input.count > 90 {
                CodeBlock(language: tool.name?.lowercased() == "code" ? "code" : "input", code: ToolSummary.pretty(input), maxLines: 30, keep: .head)
            }
            if let calls = tool.calls, !calls.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Calls · \(calls.count)")
                        .font(.system(.caption2, design: .monospaced, weight: .semibold))
                        .foregroundStyle(.secondary)
                    ForEach(Array(calls.enumerated()), id: \.offset) { _, call in
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Image(systemName: ToolBlock.callSymbol(call.state))
                                .foregroundStyle(call.state == "failed" ? .red : .secondary)
                            Text(call.name).bold()
                            VStack(alignment: .leading, spacing: 2) {
                                Text(call.gist ?? call.input ?? "").foregroundStyle(.secondary).lineLimit(1)
                                if let error = call.error, !error.isEmpty {
                                    Text(ToolSummary.firstLine(error)).foregroundStyle(.red).lineLimit(2)
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
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
            }
            if let output = tool.output, !output.isEmpty {
                CodeBlock(language: "output", code: ToolSummary.tail(output), maxLines: 60)
            }
            if let stderr = tool.stderr, !stderr.isEmpty {
                CodeBlock(language: "stderr", code: ToolSummary.tail(stderr), maxLines: 30)
            }
            if let logs = tool.logs, !logs.isEmpty {
                CodeBlock(language: "console", code: ToolSummary.tail(logs), maxLines: 30)
            }
            if let value = tool.value, !value.isEmpty {
                CodeBlock(language: "return value", code: ToolSummary.tail(ToolSummary.pretty(value)), maxLines: 30)
            }
            if let files = tool.files, !files.isEmpty {
                Label(files.joined(separator: ", "), systemImage: "doc")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if [tool.output, tool.stderr, tool.error, tool.logs, tool.value].allSatisfy({ ($0 ?? "").isEmpty }), (tool.calls ?? []).isEmpty {
                Text(state == "running" || state == "queued" ? "Waiting for output…" : "No output")
                    .font(.footnote)
                    .foregroundStyle(.tertiary)
            }
        }
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

struct ToolStateIcon: View {
    let tool: Tool?
    let live: Bool

    var body: some View {
        switch ToolBlock.state(tool, live: live) {
        case "running":
            ProgressView()
                .controlSize(.mini)
        case "queued":
            Image(systemName: "circle.dotted")
                .foregroundStyle(.secondary)
        case "done":
            if let code = tool?.exitCode, code != 0 {
                Image(systemName: "exclamationmark.circle.fill").foregroundStyle(.orange)
            } else {
                Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
            }
        case "failed":
            Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
        case "canceled":
            Image(systemName: "slash.circle").foregroundStyle(.secondary)
        default:
            // Unfinished, with no run: cut short.
            Image(systemName: "pause.circle").foregroundStyle(.secondary)
        }
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
        VStack(spacing: 6) {
            Button {
                if expandable { withAnimation(.snappy) { open.toggle() } }
            } label: {
                HStack(spacing: 4) {
                    Label(text, systemImage: "info.circle")
                        .multilineTextAlignment(.center)
                    if expandable {
                        // As the page's notice with a detail: it opens.
                        Image(systemName: "chevron.right")
                            .font(.caption2.weight(.bold))
                            .rotationEffect(.degrees(open ? 90 : 0))
                    }
                }
                .font(.footnote)
                .foregroundStyle(.secondary)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .disabled(!expandable)
            if open, let detail {
                MarkdownText(text: detail)
                    .font(.footnote)
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.fill.quaternary, in: .rect(cornerRadius: 12))
                    .transition(.opacity)
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var expandable: Bool { detail?.isEmpty == false }
}

struct ErrorBlock: View {
    let text: String
    let detail: String?
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(text, systemImage: "exclamationmark.octagon.fill")
                .font(.subheadline.weight(.medium))
                .foregroundStyle(.red)
                .textSelection(.enabled)
            if let detail, !detail.isEmpty {
                Text(detail)
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(open ? nil : 12)
                    .textSelection(.enabled)
                if detail.split(separator: "\n", omittingEmptySubsequences: false).count > 12 || detail.count > 900 {
                    Button(open ? "Show less" : "Show all") {
                        withAnimation(.snappy) { open.toggle() }
                    }
                    .font(.caption.weight(.medium))
                    .buttonStyle(.borderless)
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.red.opacity(0.1), in: .rect(cornerRadius: 14))
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

    var body: some View {
        let shown = shown
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                if let language, !language.isEmpty {
                    Text(language)
                        .font(.system(.caption2, design: .monospaced, weight: .semibold))
                        .textCase(.uppercase)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button {
                    UIPasteboard.general.string = code
                    withAnimation(.snappy) { copied = true }
                    Task {
                        try? await Task.sleep(for: .seconds(1.4))
                        withAnimation(.snappy) { copied = false }
                    }
                } label: {
                    Image(systemName: copied ? "checkmark" : "doc.on.doc")
                        .contentTransition(.symbolEffect(.replace))
                        .font(.caption)
                        .frame(width: 32, height: 26)
                        .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .foregroundStyle(copied ? AnyShapeStyle(.green) : AnyShapeStyle(.secondary))
                .accessibilityLabel(copied ? "Copied" : "Copy")
                .sensoryFeedback(.success, trigger: copied) { _, now in now }
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
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: false)
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
        .background(Color(uiColor: .secondarySystemBackground), in: .rect(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.separator.opacity(0.6), lineWidth: 0.5))
    }

    private func more(_ n: Int, _ where: String) -> some View {
        Text("… \(n) more line\(n == 1 ? "" : "s") \(`where`)")
            .font(.system(.caption2, design: .monospaced))
            .foregroundStyle(.tertiary)
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
