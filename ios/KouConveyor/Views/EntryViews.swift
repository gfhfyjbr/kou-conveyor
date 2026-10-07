import SwiftUI
import UIKit

/// EntryView shows one block of a transcript, as the cockpit's timeline
/// does: the user's prompts, the agent's answers and thinking, its tools.
struct EntryView: View {
    let entry: Entry
    /// Whether a run goes on: an unfinished tool is at work, not cut short.
    let live: Bool

    var body: some View {
        switch entry.kind {
        case Entry.user: UserBubble(entry: entry)
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

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            Text(entry.text ?? "")
                .font(.body)
                .foregroundStyle(.white)
                .textSelection(.enabled)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(Color.kouAccent.gradient, in: .rect(cornerRadius: 20))
                .opacity(entry.state == "pending" ? 0.7 : 1)
            HStack(spacing: 4) {
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
            }
            .font(.caption2)
            .foregroundStyle(.secondary)
            .labelStyle(.titleAndIcon)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.leading, 40)
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
                HStack(spacing: 6) {
                    Image(systemName: "brain")
                    Text(title)
                        .lineLimit(1)
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.bold))
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .font(.footnote.weight(.medium))
                .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            if open {
                Text(entry.text ?? "")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .padding(.leading, 22)
                    .transition(.opacity)
            }
        }
    }

    private var title: String {
        // A summary of reasoning starts with its heading: **Title**.
        let text = (entry.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if text.hasPrefix("**"), let end = text.dropFirst(2).range(of: "**") {
            return String(text[text.index(text.startIndex, offsetBy: 2)..<end.lowerBound])
        }
        return "Thinking"
    }
}

struct ToolBlock: View {
    let entry: Entry
    let live: Bool
    @State private var open = false

    var body: some View {
        let tool = entry.tool
        VStack(alignment: .leading, spacing: 10) {
            Button {
                withAnimation(.snappy) { open.toggle() }
            } label: {
                HStack(spacing: 8) {
                    ToolStateIcon(tool: tool, live: live)
                        .frame(width: 18)
                    Text(tool?.name ?? "Tool")
                        .font(.system(.subheadline, design: .monospaced, weight: .semibold))
                    Text(ToolSummary.of(tool))
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 0)
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)

            if open, let tool {
                VStack(alignment: .leading, spacing: 8) {
                    if let input = tool.input, !input.isEmpty {
                        CodeBlock(language: "input", code: ToolSummary.pretty(input), maxLines: 30)
                    }
                    if let calls = tool.calls, !calls.isEmpty {
                        ForEach(Array(calls.enumerated()), id: \.offset) { _, call in
                            HStack(spacing: 6) {
                                Image(systemName: call.state == "failed" ? "xmark.circle" : "arrow.turn.down.right")
                                    .foregroundStyle(call.state == "failed" ? .red : .secondary)
                                Text(call.name).bold()
                                Text(call.gist ?? call.input ?? "").foregroundStyle(.secondary).lineLimit(1)
                            }
                            .font(.system(.caption, design: .monospaced))
                        }
                    }
                    if let output = tool.output, !output.isEmpty {
                        CodeBlock(language: tool.exitCode.map { "exit \($0)" } ?? "output", code: ToolSummary.tail(output), maxLines: 60)
                    }
                    if let stderr = tool.stderr, !stderr.isEmpty {
                        CodeBlock(language: "stderr", code: ToolSummary.tail(stderr), maxLines: 30)
                    }
                    if let error = tool.error, !error.isEmpty {
                        Text(error)
                            .font(.footnote)
                            .foregroundStyle(.red)
                    }
                    if let files = tool.files, !files.isEmpty {
                        Label(files.joined(separator: ", "), systemImage: "doc")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .transition(.opacity)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 14))
    }
}

struct ToolStateIcon: View {
    let tool: Tool?
    let live: Bool

    var body: some View {
        switch tool?.state ?? "" {
        case "running" where live:
            ProgressView()
                .controlSize(.mini)
        case "queued" where live:
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
        guard input.hasPrefix("{"), let data = input.data(using: .utf8),
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
                if detail?.isEmpty == false { withAnimation(.snappy) { open.toggle() } }
            } label: {
                Label(text, systemImage: "info.circle")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
            .buttonStyle(.plain)
            if open, let detail {
                MarkdownText(text: detail)
                    .font(.footnote)
                    .padding(12)
                    .background(.fill.quaternary, in: .rect(cornerRadius: 12))
            }
        }
        .frame(maxWidth: .infinity)
    }
}

struct ErrorBlock: View {
    let text: String
    let detail: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(text, systemImage: "exclamationmark.octagon.fill")
                .font(.subheadline.weight(.medium))
                .foregroundStyle(.red)
            if let detail, !detail.isEmpty {
                Text(detail)
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(12)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.red.opacity(0.1), in: .rect(cornerRadius: 14))
    }
}

/// CodeBlock is code, or a tool's output: monospaced, scrolled sideways.
struct CodeBlock: View {
    let language: String?
    let code: String
    var maxLines: Int? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                if let language, !language.isEmpty {
                    Text(language)
                        .font(.system(.caption2, design: .monospaced, weight: .semibold))
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = code }
                    .labelStyle(.iconOnly)
                    .font(.caption)
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 10)
            .padding(.top, 7)
            ScrollView(.horizontal, showsIndicators: false) {
                Text(shown)
                    .font(.system(.footnote, design: .monospaced))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: false)
                    .padding(10)
            }
        }
        .background(Color(uiColor: .secondarySystemBackground), in: .rect(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.separator.opacity(0.6), lineWidth: 0.5))
    }

    private var shown: String {
        let trimmed = code.hasSuffix("\n") ? String(code.dropLast()) : code
        guard let maxLines else { return trimmed }
        let lines = trimmed.split(separator: "\n", omittingEmptySubsequences: false)
        guard lines.count > maxLines else { return trimmed }
        return lines.suffix(maxLines).joined(separator: "\n")
    }
}
