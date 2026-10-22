import SwiftUI

/// MarkdownText shows the agent's Markdown as the cockpit's page reads it
/// (plugins/markdown): paragraphs with their inline styles, headings, quotes,
/// lists (nested, numbered, tasks), code blocks and tables.
///
/// A text is parsed once: the transcript asks for its body often — as the
/// agent writes, as rows scroll back into view — and the same text gives
/// the same blocks.
struct MarkdownText: View, Equatable {
    let text: String
    /// Whether the text still grows, as the agent writes it: each of its
    /// lengths is parsed once, and kept by none.
    var streaming = false

    var body: some View {
        MarkdownBlocks(blocks: streaming ? MarkdownDocument.parse(text) : MarkdownDocument.blocks(of: text))
    }

    static func inline(_ text: String) -> AttributedString {
        MarkdownDocument.inline(text)
    }
}

/// The blocks of a text, one under another. Those of a text that streams
/// come in as they come (textReveal), the caret after the last (textCaret).
private struct MarkdownBlocks: View {
    let blocks: [MarkdownBlock]
    @Environment(\.textReveal) private var reveal
    @Environment(\.textCaret) private var caret

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { offset, block in
                MarkdownBlockView(block: block)
                    .environment(\.textCaret, caret && offset == blocks.count - 1)
                    .modifier(RiseIn(active: reveal))
            }
        }
    }
}

private struct MarkdownBlockView: View {
    let block: MarkdownBlock
    @Environment(\.textReveal) private var reveal
    @Environment(\.textCaret) private var caret

    var body: some View {
        switch block {
        case let .code(language, code):
            CodeBlock(language: language ?? "text", code: code)
        case let .heading(level, text):
            heading(level, text)
                .modifier(revealing(text, selectable: false))
        case let .quote(blocks):
            // A quote is Markdown too.
            AnyView(MarkdownBlocks(blocks: blocks))
                .foregroundStyle(.secondary)
                .padding(.leading, 12)
                .overlay(alignment: .leading) {
                    Rectangle().fill(.tertiary).frame(width: 2)
                }
        case let .paragraph(text):
            // Selectable through Reveal.
            Text(text)
                .fixedSize(horizontal: false, vertical: true)
                .modifier(revealing(text))
        case let .list(items):
            MarkdownList(items: items)
        case let .table(head, rows):
            MarkdownTable(head: head, rows: rows)
        case .rule:
            Divider()
        }
    }

    private func revealing(_ text: AttributedString, selectable: Bool = true) -> Reveal {
        Reveal(length: text.characters.count, reveal: reveal, caret: caret, selectable: selectable)
    }

    // The page's h3, h4 and h5: # and ## are titles, the rest small capitals.
    @ViewBuilder
    private func heading(_ level: Int, _ text: AttributedString) -> some View {
        switch level {
        case 1:
            Text(text)
                .font(.title3.weight(.semibold))
                .padding(.top, 6)
        case 2:
            Text(text)
                .font(.headline)
                .padding(.top, 4)
        default:
            Text(text)
                .font(.system(.caption, design: .monospaced, weight: .bold))
                .tracking(0.8)
                .textCase(.uppercase)
                .foregroundStyle(.secondary)
                .padding(.top, 6)
        }
    }
}

/// A list: its markers in a column of their own, so that a long item wraps
/// under its text and not under its marker.
private struct MarkdownList: View {
    let items: [MarkdownListItem]
    @Environment(\.textReveal) private var reveal
    @Environment(\.textCaret) private var caret

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            ForEach(Array(items.enumerated()), id: \.offset) { offset, item in
                // The caret ends the last item, or the last of its own.
                let last = caret && offset == items.count - 1
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    marker(item)
                    VStack(alignment: .leading, spacing: 5) {
                        // Selectable through Reveal.
                        Text(item.text)
                            .fixedSize(horizontal: false, vertical: true)
                            .modifier(Reveal(length: item.text.characters.count, reveal: reveal,
                                             caret: last && item.children.isEmpty))
                        if !item.children.isEmpty {
                            AnyView(MarkdownList(items: item.children))
                                .environment(\.textCaret, last)
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func marker(_ item: MarkdownListItem) -> some View {
        if let checked = item.task {
            Image(systemName: checked ? "checkmark.square" : "square")
                .font(.subheadline)
                .foregroundStyle(.secondary)
        } else if let number = item.number {
            Text("\(number).")
                .font(.system(.subheadline, design: .monospaced, weight: .semibold))
                .foregroundStyle(.secondary)
                .frame(minWidth: 18, alignment: .trailing)
        } else {
            // U+FE0E: the square as text, in its colour, not as an emoji.
            Text("▪\u{FE0E}")
                .font(.footnote)
                .foregroundStyle(.tertiary)
                .frame(minWidth: 10)
        }
    }
}

/// A table as the page draws it: a header row of small capitals, rows ruled
/// apart, scrolled sideways when it is wider than the screen.
private struct MarkdownTable: View {
    let head: [AttributedString]
    let rows: [[AttributedString]]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Grid(alignment: .topLeading, horizontalSpacing: 0, verticalSpacing: 0) {
                GridRow {
                    ForEach(Array(head.enumerated()), id: \.offset) { _, cell in
                        cellText(cell)
                            .font(.system(.caption2, design: .monospaced, weight: .bold))
                            .textCase(.uppercase)
                            .foregroundStyle(.secondary)
                            // The column's width, so the row's tint is unbroken.
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                            .background(Color.kouSurface)
                    }
                }
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    Divider()
                    GridRow {
                        ForEach(head.indices, id: \.self) { column in
                            cellText(column < row.count ? row[column] : AttributedString())
                                .font(.subheadline)
                        }
                    }
                }
            }
            .textSelection(.enabled)
            .overlay(RoundedRectangle(cornerRadius: .corner(10)).strokeBorder(.separator, lineWidth: 0.5))
            .clipShape(.rect(cornerRadius: .corner(10)))
        }
        .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
    }

    /// A cell wraps at a column's most, rather than running on: sideways,
    /// the scroll view proposes no width, and the ideal one stands in.
    private func cellText(_ text: AttributedString) -> some View {
        Text(text)
            .fixedSize(horizontal: false, vertical: true)
            .frame(idealWidth: 240, maxWidth: 240, alignment: .leading)
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
    }
}

struct MarkdownListItem: Hashable {
    let text: AttributedString
    /// The number of an ordered item.
    var number: Int?
    /// Whether a task item is done.
    var task: Bool?
    var children: [MarkdownListItem] = []
}

indirect enum MarkdownBlock: Hashable {
    case code(language: String?, code: String)
    case heading(level: Int, text: AttributedString)
    case quote([MarkdownBlock])
    case paragraph(AttributedString)
    case list([MarkdownListItem])
    case table(head: [AttributedString], rows: [[AttributedString]])
    case rule
}

/// MarkdownDocument parses Markdown into blocks as plugins/markdown does,
/// and keeps what it parsed.
@MainActor
enum MarkdownDocument {
    private final class Parsed {
        let blocks: [MarkdownBlock]
        init(_ blocks: [MarkdownBlock]) { self.blocks = blocks }
    }

    private static let cache: NSCache<NSString, Parsed> = {
        let cache = NSCache<NSString, Parsed>()
        cache.countLimit = 400
        return cache
    }()

    static func blocks(of text: String) -> [MarkdownBlock] {
        let key = text as NSString
        if let parsed = cache.object(forKey: key) { return parsed.blocks }
        let blocks = parse(text)
        cache.setObject(Parsed(blocks), forKey: key)
        return blocks
    }

    // MARK: Blocks

    nonisolated static func parse(_ source: String) -> [MarkdownBlock] {
        let lines = source.replacingOccurrences(of: "\r\n", with: "\n").split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        var blocks: [MarkdownBlock] = []
        var i = 0
        while i < lines.count {
            let line = lines[i]
            if line.trimmingCharacters(in: .whitespaces).isEmpty { i += 1; continue }

            if let open = fence(line) {
                var body: [String] = []
                i += 1
                while i < lines.count, !closes(lines[i], open.marker) {
                    body.append(lines[i])
                    i += 1
                }
                i += 1
                blocks.append(.code(language: open.language, code: body.joined(separator: "\n")))
                continue
            }
            if let title = heading(line) {
                blocks.append(.heading(level: title.level, text: inline(title.text)))
                i += 1
                continue
            }
            if isRule(line) {
                blocks.append(.rule)
                i += 1
                continue
            }
            if quoteBody(line) != nil {
                var body: [String] = []
                while i < lines.count, let text = quoteBody(lines[i]) {
                    body.append(text)
                    i += 1
                }
                blocks.append(.quote(parse(body.joined(separator: "\n"))))
                continue
            }
            if item(line) != nil {
                let start = i
                while i < lines.count {
                    let next = lines[i]
                    if item(next) != nil { i += 1; continue }
                    // Continuation lines are indented; a blank line ends the
                    // list unless another item follows.
                    let blank = next.trimmingCharacters(in: .whitespaces).isEmpty
                    if !blank, indent(of: next) >= 2 { i += 1; continue }
                    if blank, i + 1 < lines.count, item(lines[i + 1]) != nil { i += 1; continue }
                    break
                }
                blocks.append(.list(list(Array(lines[start..<i]))))
                continue
            }
            if line.contains("|"), i + 1 < lines.count, isTableRule(lines[i + 1]) {
                let head = cells(line)
                var rows: [[AttributedString]] = []
                i += 2
                while i < lines.count, lines[i].contains("|"), !lines[i].trimmingCharacters(in: .whitespaces).isEmpty {
                    rows.append(cells(lines[i]).map(inline))
                    i += 1
                }
                blocks.append(.table(head: head.map(inline), rows: rows))
                continue
            }
            var paragraph: [String] = []
            while i < lines.count, !lines[i].trimmingCharacters(in: .whitespaces).isEmpty {
                let next = lines[i]
                if !paragraph.isEmpty, fence(next) != nil || heading(next) != nil || quoteBody(next) != nil || item(next) != nil || isRule(next) { break }
                paragraph.append(next.trimmingCharacters(in: .whitespaces))
                i += 1
            }
            blocks.append(.paragraph(inline(paragraph.joined(separator: "\n"))))
        }
        return blocks
    }

    private nonisolated static func indent(of line: String) -> Int {
        var n = 0
        for c in line {
            if c == " " { n += 1 } else if c == "\t" { n += 4 } else { break }
        }
        return n
    }

    /// An opening fence: ``` or ~~~, three or more, and a language.
    private nonisolated static func fence(_ line: String) -> (marker: String, language: String?)? {
        guard indent(of: line) <= 3 else { return nil }
        let trimmed = line.drop { $0 == " " }
        guard let first = trimmed.first, first == "`" || first == "~" else { return nil }
        let marker = trimmed.prefix { $0 == first }
        guard marker.count >= 3 else { return nil }
        let language = trimmed.dropFirst(marker.count).trimmingCharacters(in: .whitespaces)
            .prefix { $0.isLetter || $0.isNumber || "_+#.-".contains($0) }
        return (String(marker), language.isEmpty ? nil : String(language))
    }

    /// Whether a line closes a fence: its character, at least as many.
    private nonisolated static func closes(_ line: String, _ marker: String) -> Bool {
        guard indent(of: line) <= 3, let char = marker.first else { return false }
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        return trimmed.count >= marker.count && trimmed.allSatisfy { $0 == char }
    }

    private nonisolated static func heading(_ line: String) -> (level: Int, text: String)? {
        guard indent(of: line) <= 3 else { return nil }
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        let hashes = trimmed.prefix { $0 == "#" }.count
        guard (1...6).contains(hashes) else { return nil }
        let rest = trimmed.dropFirst(hashes)
        guard rest.isEmpty || rest.first == " " else { return nil }
        // Closing hashes are not part of the title.
        var text = rest.trimmingCharacters(in: .whitespaces)
        while text.hasSuffix("#") { text.removeLast() }
        return (hashes, text.trimmingCharacters(in: .whitespaces))
    }

    /// ---, ***, ___, and spaced: - - -.
    private nonisolated static func isRule(_ line: String) -> Bool {
        guard indent(of: line) <= 3 else { return false }
        let marks = line.filter { $0 != " " && $0 != "\t" }
        guard marks.count >= 3, let first = marks.first, "-*_".contains(first) else { return false }
        return marks.allSatisfy { $0 == first }
    }

    private nonisolated static func quoteBody(_ line: String) -> String? {
        guard indent(of: line) <= 3 else { return nil }
        let trimmed = line.drop { $0 == " " }
        guard trimmed.first == ">" else { return nil }
        var rest = trimmed.dropFirst()
        if rest.first == " " { rest = rest.dropFirst() }
        return String(rest)
    }

    /// A list item: its indent, its number if ordered, and its text.
    private nonisolated static func item(_ line: String) -> (indent: Int, number: Int?, text: String)? {
        let indent = indent(of: line)
        let rest = line.drop { $0 == " " || $0 == "\t" }
        if let first = rest.first, "-*+".contains(first) {
            let after = rest.dropFirst()
            guard let space = after.first, space == " " || space == "\t" else { return nil }
            return (indent, nil, after.trimmingCharacters(in: .whitespaces))
        }
        let digits = rest.prefix { $0.isASCII && $0.isNumber }
        guard (1...9).contains(digits.count) else { return nil }
        let after = rest.dropFirst(digits.count)
        guard let dot = after.first, dot == "." || dot == ")" else { return nil }
        let text = after.dropFirst()
        guard let space = text.first, space == " " || space == "\t" else { return nil }
        return (indent, Int(digits), text.trimmingCharacters(in: .whitespaces))
    }

    /// The items of a list, nested under the closest previous item with a
    /// smaller indent.
    private nonisolated static func list(_ lines: [String]) -> [MarkdownListItem] {
        final class Node {
            let indent: Int
            let number: Int?
            var text: [String]
            var children: [Node] = []
            init(indent: Int, number: Int?, text: String) {
                self.indent = indent
                self.number = number
                self.text = [text]
            }
        }
        var roots: [Node] = []
        var stack: [Node] = []
        for line in lines {
            guard let found = item(line) else {
                let trimmed = line.trimmingCharacters(in: .whitespaces)
                if let last = stack.last, !trimmed.isEmpty { last.text.append(trimmed) }
                continue
            }
            let node = Node(indent: found.indent, number: found.number, text: found.text)
            while let top = stack.last, found.indent <= top.indent { stack.removeLast() }
            if let parent = stack.last { parent.children.append(node) } else { roots.append(node) }
            stack.append(node)
        }
        func items(_ nodes: [Node]) -> [MarkdownListItem] {
            nodes.map { node in
                var text = node.text.joined(separator: "\n")
                var task: Bool?
                for (box, done) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] where text.hasPrefix(box) {
                    task = done
                    text = String(text.dropFirst(box.count)).trimmingCharacters(in: .whitespaces)
                }
                return MarkdownListItem(text: inline(text), number: task == nil ? node.number : nil, task: task, children: items(node.children))
            }
        }
        return items(roots)
    }

    private nonisolated static func isTableRule(_ line: String) -> Bool {
        var row = line.trimmingCharacters(in: .whitespaces)
        if row.hasPrefix("|") { row.removeFirst() }
        if row.hasSuffix("|") { row.removeLast() }
        let cells = row.split(separator: "|", omittingEmptySubsequences: false)
        guard !cells.isEmpty else { return false }
        return cells.allSatisfy { cell in
            var c = cell.trimmingCharacters(in: .whitespaces)
            if c.hasPrefix(":") { c.removeFirst() }
            if c.hasSuffix(":") { c.removeLast() }
            return c.count >= 2 && c.allSatisfy { $0 == "-" }
        }
    }

    /// A table row's cells; \| is a bar within a cell.
    private nonisolated static func cells(_ line: String) -> [String] {
        var row = line.trimmingCharacters(in: .whitespaces)
        if row.hasPrefix("|") { row.removeFirst() }
        if row.hasSuffix("|"), !row.hasSuffix("\\|") { row.removeLast() }
        var cells: [String] = []
        var cell = ""
        var escaped = false
        for c in row {
            if escaped {
                if c != "|" { cell.append("\\") }
                cell.append(c)
                escaped = false
            } else if c == "\\" {
                escaped = true
            } else if c == "|" {
                cells.append(cell.trimmingCharacters(in: .whitespaces))
                cell = ""
            } else {
                cell.append(c)
            }
        }
        if escaped { cell.append("\\") }
        cells.append(cell.trimmingCharacters(in: .whitespaces))
        return cells
    }

    // MARK: Inline

    private nonisolated static let bareURL = try! NSRegularExpression(pattern: #"https?://[^\s<>()]*[^\s<>().,;:!?'"\]]"#)

    /// Inline Markdown — code, strong, emphasis, strikethrough, links — with
    /// bare http(s) addresses linked and code set apart, as on the page.
    nonisolated static func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace, failurePolicy: .returnPartiallyParsedIfPossible)
        var string = (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)

        // Code: a tinted chip, a little smaller than the text around it.
        for run in string.runs where run.inlinePresentationIntent?.contains(.code) == true {
            string[run.range].swiftUI.backgroundColor = Color.secondary.opacity(0.14)
        }

        // Bare addresses: linked, unless already in a link or in code.
        let plain = String(string.characters)
        guard plain.contains("http") else { return string }
        let matches = bareURL.matches(in: plain, range: NSRange(plain.startIndex..., in: plain))
        for match in matches.reversed() {
            guard let range = Range(match.range, in: string),
                  let url = URL(string: String(string[range].characters))
            else { continue }
            let untouched = string[range].runs.allSatisfy { $0.link == nil && $0.inlinePresentationIntent?.contains(.code) != true }
            if untouched { string[range].link = url }
        }
        return string
    }
}
