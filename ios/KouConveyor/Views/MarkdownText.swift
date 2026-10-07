import SwiftUI

/// MarkdownText shows the agent's Markdown: paragraphs and lists with their
/// inline styles, headings, quotes, code blocks and tables, which keep their
/// columns in a monospaced block.
struct MarkdownText: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(MarkdownBlock.parse(text).enumerated()), id: \.offset) { _, block in
                switch block {
                case let .code(language, code):
                    CodeBlock(language: language, code: code)
                case let .heading(level, text):
                    Text(Self.inline(text))
                        .font(level <= 1 ? .title3.weight(.bold) : level == 2 ? .headline : .subheadline.weight(.semibold))
                        .padding(.top, 4)
                case let .quote(text):
                    Text(Self.inline(text))
                        .foregroundStyle(.secondary)
                        .padding(.leading, 12)
                        .overlay(alignment: .leading) {
                            Capsule().fill(.tertiary).frame(width: 3)
                        }
                case let .paragraph(text):
                    Text(Self.inline(text))
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                case .rule:
                    Divider()
                }
            }
        }
    }

    static func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace, failurePolicy: .returnPartiallyParsedIfPossible)
        return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
    }
}

enum MarkdownBlock: Equatable {
    case code(language: String?, code: String)
    case heading(level: Int, text: String)
    case quote(String)
    case paragraph(String)
    case rule

    /// parse splits Markdown into its blocks.
    static func parse(_ text: String) -> [MarkdownBlock] {
        var blocks: [MarkdownBlock] = []
        var paragraph: [String] = []
        var quote: [String] = []
        var table: [String] = []
        var fence: (marker: String, language: String?, lines: [String])?

        func flush() {
            if !paragraph.isEmpty {
                blocks.append(.paragraph(paragraph.joined(separator: "\n")))
                paragraph = []
            }
            if !quote.isEmpty {
                blocks.append(.quote(quote.joined(separator: "\n")))
                quote = []
            }
            if !table.isEmpty {
                blocks.append(.code(language: nil, code: table.joined(separator: "\n")))
                table = []
            }
        }

        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(raw)
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if var open = fence {
                if trimmed.hasPrefix(open.marker) {
                    blocks.append(.code(language: open.language, code: open.lines.joined(separator: "\n")))
                    fence = nil
                } else {
                    open.lines.append(line)
                    fence = open
                }
                continue
            }
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                flush()
                let marker = String(trimmed.prefix(3))
                let language = trimmed.dropFirst(3).trimmingCharacters(in: .whitespaces)
                fence = (marker, language.isEmpty ? nil : language, [])
                continue
            }
            if trimmed.isEmpty {
                flush()
                continue
            }
            if trimmed.hasPrefix("|") {
                if !paragraph.isEmpty || !quote.isEmpty {
                    let pending = table
                    table = []
                    flush()
                    table = pending
                }
                table.append(trimmed)
                continue
            } else if !table.isEmpty {
                flush()
            }
            if let heading = heading(trimmed) {
                flush()
                blocks.append(heading)
                continue
            }
            if trimmed == "---" || trimmed == "***" || trimmed == "___" {
                flush()
                blocks.append(.rule)
                continue
            }
            if trimmed.hasPrefix(">") {
                if !paragraph.isEmpty { flush() }
                quote.append(String(trimmed.dropFirst()).trimmingCharacters(in: .whitespaces))
                continue
            } else if !quote.isEmpty {
                flush()
            }
            paragraph.append(listItem(line))
        }
        if let open = fence {
            blocks.append(.code(language: open.language, code: open.lines.joined(separator: "\n")))
        }
        flush()
        return blocks
    }

    private static func heading(_ line: String) -> MarkdownBlock? {
        let hashes = line.prefix { $0 == "#" }.count
        guard (1...6).contains(hashes), line.dropFirst(hashes).first == " " else { return nil }
        return .heading(level: hashes, text: line.dropFirst(hashes + 1).trimmingCharacters(in: .whitespaces))
    }

    /// A list item's marker as a bullet, its indent kept.
    private static func listItem(_ line: String) -> String {
        let indent = line.prefix { $0 == " " || $0 == "\t" }
        let rest = line.dropFirst(indent.count)
        for marker in ["- ", "* ", "+ "] where rest.hasPrefix(marker) {
            let depth = indent.count / 2
            return String(repeating: "   ", count: depth) + "•  " + rest.dropFirst(2)
        }
        if indent.count >= 2 { return String(repeating: "   ", count: indent.count / 2) + rest }
        return String(rest)
    }
}
