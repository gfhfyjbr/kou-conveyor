import Charts
import SwiftUI
import UIKit

/// SurfaceBlock is a UI call of the genui plugin (examples/plugins/genui) in
/// the transcript: the surface it showed, drawn with the app's own controls,
/// as the web cockpit draws it with its own. A surface's newest call is
/// live — its files read as they change, its inputs and buttons at work —
/// and the calls before it are snapshots, closed until tapped: what the
/// call showed, the files as it read them.
struct SurfaceBlock: View {
    let entry: Entry
    let call: SurfaceCall
    let book: SurfaceBook
    let context: EntryContext
    /// Whether it is open; nil: the newest call is, the others are not.
    @State private var open: Bool?
    /// The file of the surface shown over the screen.
    @State private var opened: SurfaceOpened?

    var body: some View {
        let head = book.isHead(entry.id, call.surface)
        let live = head && !call.deleted
        let shown = !call.deleted && (open ?? live)
        let load = book.versions[call.path]
        VStack(alignment: .leading, spacing: 10) {
            header(load: load, live: live, shown: shown)
            if shown {
                content(load: load, live: live)
                    .transition(.opacity)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .kouPanel(radius: 14)
        .task(id: "\(call.path)|\(shown || head)") {
            if (shown || head) && !call.deleted { await book.load(call.path) }
        }
        .sheet(item: $opened) { file in
            NavigationStack {
                FileScreen(client: context.client, workspace: book.workspace, path: file.path)
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Done") { opened = nil }
                        }
                    }
            }
        }
    }

    private var version: SurfaceVersion? {
        if case let .ready(version)? = book.versions[call.path] { return version }
        return nil
    }

    private var title: String {
        if let title = version?.title, !title.isEmpty { return title }
        return call.title.isEmpty ? call.surface : call.title
    }

    private func header(load: SurfaceLoad?, live: Bool, shown: Bool) -> some View {
        Button {
            withAnimation(.snappy) { open = !shown }
        } label: {
            HStack(spacing: 8) {
                Image(systemName: call.deleted ? "rectangle.slash" : "rectangle.3.group")
                    .font(.subheadline)
                    .foregroundStyle(live ? Color.kouAccent : Color.kouFg3)
                    .frame(width: 18)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(Color.kouFg)
                        .lineLimit(1)
                    Text(subtitle(load: load, live: live, shown: shown))
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg3)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .layoutPriority(1)
                Spacer(minLength: 0)
                chip(load: load, live: live)
                if !call.deleted {
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(Color.kouFg4)
                        .rotationEffect(.degrees(shown ? 90 : 0))
                }
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .disabled(call.deleted)
        .contextMenu {
            Button("Copy as text", systemImage: "doc.on.doc") {
                UIPasteboard.general.string = version?.text ?? call.text
            }
            if !call.deleted {
                Button("Open the version file", systemImage: "doc.text") {
                    opened = SurfaceOpened(path: call.path)
                }
            }
            if live, book.edited(call.path) {
                Button("Reset the inputs", systemImage: "arrow.counterclockwise") {
                    book.reset(call.path)
                }
            }
        }
    }

    /// subtitle says what the surface is: its state while live, as the web
    /// card's head does, and the glance when it is closed.
    private func subtitle(load: SurfaceLoad?, live: Bool, shown: Bool) -> String {
        if call.deleted { return "removed (v\(call.version))" }
        switch load {
        case let .failed(message)?: return message
        case let .ready(version)?:
            if live {
                let state = status(version)
                if !shown, !version.glance.isEmpty {
                    let doc = book.doc(call, version, live: true)
                    return "\(SurfaceText.fill(doc, version.components, version.glance)) · \(state)"
                }
                return state
            }
            return snapshotLine
        default:
            return live ? "v\(call.version)" : snapshotLine
        }
    }

    private var snapshotLine: String {
        var line = "v\(call.version) · snapshot"
        if let newest = book.newest(call.surface), newest.version != call.version || newest.path != call.path {
            line += newest.deleted ? " → removed" : " → v\(newest.version)"
        }
        return line
    }

    /// status is a live surface's: a source that does not read, the files
    /// it waits for, or when it last changed.
    private func status(_ version: SurfaceVersion) -> String {
        let read = book.live[call.path] ?? [:]
        if version.sources.isEmpty {
            return "v\(version.version)" + (version.at.map { " · " + Format.time($0) } ?? "")
        }
        if let broken = version.sources.first(where: { spec in
            guard let error = read[spec.name]?.error else { return false }
            return error != "no such file"
        }) {
            return "\(broken.file): \(read[broken.name]?.error ?? "")"
        }
        let waiting = version.sources.filter { read[$0.name]?.error == "no such file" }
        if let first = waiting.first {
            return "waiting for \(first.file)" + (waiting.count > 1 ? " +\(waiting.count - 1)" : "")
        }
        let changed = version.sources.compactMap { read[$0.name]?.modified }.max() ?? version.at
        return "live" + (changed.map { " · " + Format.time($0) } ?? "")
    }

    @ViewBuilder
    private func chip(load: SurfaceLoad?, live: Bool) -> some View {
        let failed: Bool = if case .failed? = load { true } else { false }
        let word = call.deleted ? "Removed" : failed ? "Unreadable" : live ? "Live" : "Snapshot"
        // The cockpit's card head: a square in the state's colour, the
        // word in mono capitals.
        StateMark(word: word, tone: failed ? .kouErr : live ? .kouOK : .kouFg3)
    }

    @ViewBuilder
    private func content(load: SurfaceLoad?, live: Bool) -> some View {
        switch load {
        case let .ready(version)?:
            let env = SurfaceEnv(book: book, call: call, version: version, doc: book.doc(call, version, live: live),
                                 live: live, client: context.client, workspace: book.workspace,
                                 open: { path, _ in opened = SurfaceOpened(path: path) })
            VStack(alignment: .leading, spacing: 12) {
                if version.components["root"] == nil {
                    SurfaceNone(text: "This surface has no root component.")
                } else {
                    SurfaceNode(env: env, id: "root", depth: 0)
                }
                footer(env)
            }
        case let .failed(message)?:
            VStack(alignment: .leading, spacing: 6) {
                Label(message, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(Color.kouWarn)
                if !call.text.isEmpty {
                    Text(call.text)
                        .font(.system(.caption2, design: .monospaced))
                        .foregroundStyle(Color.kouFg3)
                        .lineLimit(40)
                }
            }
        default:
            HStack(spacing: 8) {
                ProgressView()
                    .controlSize(.small)
                Text("Reading the surface…")
                    .font(.caption)
                    .foregroundStyle(Color.kouFg3)
            }
        }
    }

    /// footer is what the surface reads, and what the call warned of.
    private func footer(_ env: SurfaceEnv) -> some View {
        let sources = env.doc.object?.object("sources")
        let info = env.doc.object?.object("sourceInfo")
        let warnings = call.warnings
        return VStack(alignment: .leading, spacing: 2) {
            ForEach(env.version.sources, id: \.name) { spec in
                Text(Self.sourceLine(spec, value: sources?[spec.name], info: info?.object(spec.name)))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            ForEach(Array(warnings.prefix(3).enumerated()), id: \.offset) { _, warning in
                Text("⚠ " + warning)
                    .foregroundStyle(Color.kouWarn)
                    .lineLimit(2)
            }
            Text("\(call.surface) v\(call.version)")
        }
        .font(.system(.caption2, design: .monospaced))
        .foregroundStyle(Color.kouFg4)
    }

    private static func sourceLine(_ spec: SurfaceSourceSpec, value: SurfaceValue?, info: SurfaceObject?) -> String {
        let what: String
        if let error = info?.string("error") {
            what = error == "no such file" ? "no file yet" : error
        } else if let count = value?.object?.int("count") {
            what = "\(SurfaceText.num(Double(count))) \(count == 1 ? "row" : "rows")"
        } else if value == nil {
            what = "no file yet"
        } else if let size = info?.number("size") {
            what = Format.bytes(Int64(size))
        } else {
            what = "read"
        }
        return "\(spec.name) \(spec.file) · \(what)"
    }
}

/// A file of a surface, shown over the screen.
struct SurfaceOpened: Identifiable {
    let path: String
    var id: String { path }
}

/// SurfaceEnv is what the components of a surface are drawn from.
struct SurfaceEnv {
    let book: SurfaceBook
    let call: SurfaceCall
    let version: SurfaceVersion
    /// The data model: data, inputs and sources.
    let doc: SurfaceValue
    /// Whether inputs and buttons work: a snapshot's do not.
    let live: Bool
    let client: RelayClient
    let workspace: String
    /// open shows a file of the workspace, at a line.
    let open: (String, Int?) -> Void

    var comps: [String: SurfaceObject] { version.components }
    var path: String { call.path }

    func text(_ value: SurfaceValue?) -> String { SurfaceText.textOf(doc, comps, value) }
    func label(_ c: SurfaceObject) -> String { text(c["label"]) }
    func resolve(_ value: SurfaceValue?) -> SurfaceValue? { SurfacePointer.resolve(doc, value) }
}

/// SurfaceNode is a component of a surface, by its id.
struct SurfaceNode: View {
    let env: SurfaceEnv
    let id: String
    let depth: Int

    var body: some View {
        if depth <= 32, let c = env.comps[id] {
            SurfaceKinds.draw(env, c, depth)
        }
    }
}

/// SurfaceNodes are the children of a container, one under another.
struct SurfaceNodes: View {
    let env: SurfaceEnv
    let ids: [String]
    let depth: Int

    var body: some View {
        ForEach(Array(ids.enumerated()), id: \.offset) { _, id in
            SurfaceNode(env: env, id: id, depth: depth + 1)
        }
    }
}

enum SurfaceKinds {
    @MainActor
    static func draw(_ env: SurfaceEnv, _ c: SurfaceObject, _ depth: Int) -> AnyView {
        switch c.string("component") ?? "" {
        case "Column": AnyView(SurfaceColumn(env: env, c: c, depth: depth))
        case "Row": AnyView(SurfaceRow(env: env, c: c, depth: depth))
        case "Grid": AnyView(SurfaceGrid(env: env, c: c, depth: depth))
        case "Section": AnyView(SurfaceSection(env: env, c: c, depth: depth))
        case "Tabs": AnyView(SurfaceTabs(env: env, c: c, depth: depth))
        case "Disclosure": AnyView(SurfaceDisclosure(env: env, c: c, depth: depth))
        case "Text": AnyView(SurfaceTextView(env: env, c: c))
        case "Markdown": AnyView(MarkdownText(text: env.text(c["text"])))
        case "Metric": AnyView(SurfaceMetricView(env: env, c: c))
        case "Badge": AnyView(SurfaceBadge(text: env.text(c["text"]), tone: c.string("tone")))
        case "Progress": AnyView(SurfaceProgress(env: env, c: c))
        case "Table": AnyView(SurfaceTable(env: env, c: c))
        case "List": AnyView(SurfaceList(env: env, c: c))
        case "Chart": AnyView(SurfaceChart(env: env, c: c))
        case "Image": AnyView(SurfaceImage(env: env, c: c))
        case "Code": AnyView(SurfaceCode(env: env, c: c))
        case "Diff": AnyView(SurfaceDiff(env: env, c: c))
        case "FileLink": AnyView(SurfaceFileLink(env: env, c: c))
        case "Button": AnyView(SurfaceButton(env: env, c: c))
        case "TextField": AnyView(SurfaceTextField(env: env, c: c))
        case "Toggle": AnyView(SurfaceToggle(env: env, c: c))
        case "Select": AnyView(SurfaceSelect(env: env, c: c))
        case "Slider": AnyView(SurfaceSlider(env: env, c: c))
        case "Html": AnyView(SurfaceHtml(env: env, c: c))
        default: AnyView(SurfaceUnknown(kind: c.string("component") ?? "?", alt: c.string("alt") ?? "not drawn here"))
        }
    }

    /// Kinds small enough to stand side by side in a row.
    static let small: Set<String> = ["Metric", "Badge", "Button", "FileLink", "Text"]

    static func gap(_ c: SurfaceObject) -> CGFloat {
        switch c.string("gap") {
        case "none": 0
        case "small": 6
        case "large": 18
        default: 10
        }
    }

    static func children(_ c: SurfaceObject) -> [String] {
        (c.array("children") ?? []).compactMap(\.string)
    }

    /// The colours of a chart's series: the accent first, as the cockpit's.
    static var series: [Color] {
        [Color.kouAccent, Color(red: 0x6f / 255, green: 0x8c / 255, blue: 0xff / 255),
         Color(red: 0x3f / 255, green: 0xb6 / 255, blue: 0xa8 / 255), Color(red: 0xb5 / 255, green: 0x8c / 255, blue: 0xf0 / 255),
         Color(red: 0xd9 / 255, green: 0xa9 / 255, blue: 0x3a / 255), Color(red: 0xe0 / 255, green: 0x67 / 255, blue: 0x9b / 255)]
    }

    /// jsString is a value as JavaScript's String() writes it.
    static func jsString(_ value: SurfaceValue) -> String {
        switch value {
        case .null: "null"
        case let .bool(b): b ? "true" : "false"
        case let .number(n): SurfaceJSON.number(n)
        case let .string(s): s
        case .array, .object: SurfaceJSON.encode(value)
        }
    }

    static func align(_ name: String?) -> HorizontalAlignment {
        switch name {
        case "right": .trailing
        case "center": .center
        default: .leading
        }
    }
}

// MARK: Layout

struct SurfaceColumn: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    let depth: Int

    var body: some View {
        VStack(alignment: .leading, spacing: SurfaceKinds.gap(c)) {
            SurfaceNodes(env: env, ids: SurfaceKinds.children(c), depth: depth)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A Row of Metrics reads as one strip; one of small things wraps as a
/// line of text does; one of larger things stands them one under another,
/// as a phone has the width of one.
struct SurfaceRow: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    let depth: Int

    var body: some View {
        let ids = SurfaceKinds.children(c)
        let kinds = ids.compactMap { env.comps[$0]?.string("component") }
        let gap = SurfaceKinds.gap(c)
        if kinds.count > 1, kinds.allSatisfy({ $0 == "Metric" }) {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 96), spacing: 14, alignment: .topLeading)], alignment: .leading, spacing: 12) {
                SurfaceNodes(env: env, ids: ids, depth: depth)
            }
            .padding(12)
            .background(Color.kouSurface, in: .rect(cornerRadius: .corner(10)))
        } else if !kinds.isEmpty, kinds.allSatisfy({ SurfaceKinds.small.contains($0) }) {
            SurfaceFlow(spacing: max(gap, 6), lineSpacing: max(gap, 6)) {
                SurfaceNodes(env: env, ids: ids, depth: depth)
            }
        } else {
            VStack(alignment: .leading, spacing: gap) {
                SurfaceNodes(env: env, ids: ids, depth: depth)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct SurfaceGrid: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    let depth: Int
    @Environment(\.horizontalSizeClass) private var size

    var body: some View {
        let gap = SurfaceKinds.gap(c)
        let columns: [GridItem] = if let n = c.int("columns"), n > 0 {
            Array(repeating: GridItem(.flexible(), spacing: gap, alignment: .topLeading), count: min(n, size == .compact ? 3 : 12))
        } else {
            [GridItem(.adaptive(minimum: 150), spacing: gap, alignment: .topLeading)]
        }
        LazyVGrid(columns: columns, alignment: .leading, spacing: gap) {
            SurfaceNodes(env: env, ids: SurfaceKinds.children(c), depth: depth)
        }
    }
}

struct SurfaceSection: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    let depth: Int

    var body: some View {
        let title = env.text(c["title"])
        VStack(alignment: .leading, spacing: 8) {
            if !title.isEmpty {
                Text(title)
                    .font(.subheadline.weight(.semibold))
            }
            VStack(alignment: .leading, spacing: SurfaceKinds.gap(c)) {
                SurfaceNodes(env: env, ids: SurfaceKinds.children(c), depth: depth)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct SurfaceTabs: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    let depth: Int
    @State private var at = 0

    var body: some View {
        let tabs = (c.array("tabs") ?? []).compactMap(\.object)
        if !tabs.isEmpty {
            let current = min(at, tabs.count - 1)
            let labels = tabs.enumerated().map { i, tab in tab["label"].map { env.text($0) } ?? "Tab \(i + 1)" }
            VStack(alignment: .leading, spacing: 10) {
                if tabs.count <= 4 {
                    Picker("Tab", selection: $at) {
                        ForEach(labels.indices, id: \.self) { i in
                            Text(labels[i]).tag(i)
                        }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                } else {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 6) {
                            ForEach(labels.indices, id: \.self) { i in
                                Button(labels[i]) { at = i }
                                    .font(.footnote.weight(i == current ? .semibold : .regular))
                                    .foregroundStyle(i == current ? Color.kouAccent : Color.kouFg2)
                                    .padding(.horizontal, 10)
                                    .padding(.vertical, 5)
                                    .background(i == current ? Color.kouAccentSoft : Color.kouChip,
                                                in: .cornerCapsule)
                                    .buttonStyle(.plain)
                            }
                        }
                    }
                }
                if let child = tabs[current].string("child") {
                    SurfaceNode(env: env, id: child, depth: depth + 1)
                }
            }
        }
    }
}

struct SurfaceDisclosure: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    let depth: Int
    @State private var open: Bool?

    var body: some View {
        let isOpen = open ?? (c.bool("open") == true)
        VStack(alignment: .leading, spacing: 8) {
            Button {
                withAnimation(.snappy) { open = !isOpen }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(Color.kouFg3)
                        .rotationEffect(.degrees(isOpen ? 90 : 0))
                    Text(env.label(c))
                        .font(.subheadline.weight(.medium))
                    Spacer(minLength: 0)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if isOpen {
                VStack(alignment: .leading, spacing: SurfaceKinds.gap(c)) {
                    SurfaceNodes(env: env, ids: SurfaceKinds.children(c), depth: depth)
                }
                .padding(.leading, 14)
                .transition(.opacity)
            }
        }
    }
}

/// SurfaceFlow lays its views out as words: side by side, wrapping to the
/// next line where the width ends.
struct SurfaceFlow: Layout {
    var spacing: CGFloat = 8
    var lineSpacing: CGFloat = 8

    private func sizes(_ subviews: Subviews, width: CGFloat) -> [CGSize] {
        subviews.map { view in
            let ideal = view.sizeThatFits(.unspecified)
            guard ideal.width > width else { return ideal }
            return CGSize(width: width, height: view.sizeThatFits(ProposedViewSize(width: width, height: nil)).height)
        }
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0
        var y: CGFloat = 0
        var line: CGFloat = 0
        var widest: CGFloat = 0
        for size in sizes(subviews, width: width) {
            if x > 0, x + size.width > width {
                y += line + lineSpacing
                x = 0
                line = 0
            }
            x += size.width + spacing
            line = max(line, size.height)
            widest = max(widest, x - spacing)
        }
        return CGSize(width: width.isFinite ? min(widest, width) : widest, height: y + line)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX
        var y = bounds.minY
        var line: CGFloat = 0
        for (view, size) in zip(subviews, sizes(subviews, width: bounds.width)) {
            if x > bounds.minX, x + size.width > bounds.maxX {
                y += line + lineSpacing
                x = bounds.minX
                line = 0
            }
            view.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(width: size.width, height: size.height))
            x += size.width + spacing
            line = max(line, size.height)
        }
    }
}

// MARK: Text and values

struct SurfaceTextView: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    var body: some View {
        let text = env.text(c["text"])
        Group {
            switch c.string("style") {
            case "heading": Text(text).font(.headline)
            case "caption": Text(text).font(.caption).foregroundStyle(Color.kouFg3)
            case "mono": Text(text).font(.system(.footnote, design: .monospaced)).foregroundStyle(Color.kouFg2)
            default: Text(text).font(.subheadline)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
    }
}

struct SurfaceMetricView: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    var body: some View {
        let m = SurfaceText.metric(env.doc, c)
        // The cockpit's metric: its name in mono capitals, the number in
        // mono, the change under it in its tone.
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow(env.label(c))
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(m.text)
                    .font(.system(.title3, design: .monospaced, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(m.missing ? Color.kouFg4 : Color.kouFg)
                if !m.unit.isEmpty {
                    Text(m.unit)
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg3)
                }
            }
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            Text(m.change?.text ?? " ")
                .font(.system(.caption2, design: .monospaced, weight: .medium))
                .monospacedDigit()
                .foregroundStyle(Self.color(m.tone))
        }
        .accessibilityElement(children: .combine)
    }

    static func color(_ tone: SurfaceMetric.Tone) -> Color {
        switch tone {
        case .good: .kouOK
        case .bad: .kouErr
        case .flat: .kouFg3
        }
    }
}

struct SurfaceBadge: View {
    let text: String
    let tone: String?

    /// genui's info: its second series' blue, a little toward the text.
    private static let info = Color(uiColor: Look.dynamic(light: "#5b72cb", dark: "#8aa1f9"))

    var body: some View {
        // As the cockpit's badge: mono capitals in a box of the tone's.
        let color: Color = switch tone {
        case "ok": .kouOK
        case "warn": .kouWarn
        case "err": .kouErr
        case "info": Self.info
        default: .kouFg2
        }
        Tag(text, tone: color)
    }
}

struct SurfaceProgress: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    var body: some View {
        let value = env.resolve(c["value"]).flatMap(SurfaceText.asNumber)
        let top = env.resolve(c["max"] ?? .number(1)).flatMap(SurfaceText.asNumber).flatMap { $0 == 0 ? nil : $0 } ?? 1
        let share = value.map { max(0, min(1, $0 / top)) } ?? 0
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(env.label(c))
                    .font(.caption)
                    .foregroundStyle(Color.kouFg2)
                    .lineLimit(1)
                Spacer()
                Text(value == nil ? "—" : "\(Int((share * 100).rounded()))%")
                    .font(.system(.caption, design: .monospaced, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(value == nil ? Color.kouFg4 : Color.kouFg)
            }
            // The cockpit's bar: the accent on a track, square-ended.
            Meter(value: share)
                .frame(height: 6)
        }
        .accessibilityElement(children: .combine)
    }
}

struct SurfaceNone: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.caption)
            .foregroundStyle(Color.kouFg3)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct SurfaceUnknown: View {
    let kind: String
    let alt: String
    var note: String? = nil

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            SurfaceBadge(text: kind, tone: "neutral")
            VStack(alignment: .leading, spacing: 2) {
                Text(alt)
                    .font(.subheadline)
                if let note {
                    Text(note)
                        .font(.caption)
                        .foregroundStyle(Color.kouFg3)
                }
            }
        }
    }
}

// MARK: Tables and lists

struct SurfaceTable: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    struct Column {
        let key: String
        let label: String
        var align: String?
    }

    var body: some View {
        let rows = env.resolve(c["rows"])
        if let rows = rows?.array {
            table(rows)
        } else {
            SurfaceNone(text: rows == nil ? "No rows yet" : "Not a list: \(SurfaceText.shown(rows))")
        }
    }

    private func cell(_ row: SurfaceValue, _ column: Column, _ i: Int) -> SurfaceValue? {
        if let o = row.object { return o[column.key] }
        return i == 0 ? row : nil
    }

    @ViewBuilder
    private func table(_ rows: [SurfaceValue]) -> some View {
        let reverse = c.bool("reverse") == true
        let ordered = reverse ? Array(rows.reversed()) : rows
        let limit = c.int("limit").flatMap { $0 > 0 ? $0 : nil } ?? 10
        let shown = Array(ordered.prefix(limit))
        let columns = self.columns(rows, shown)
        let more = rows.count - shown.count
        VStack(alignment: .leading, spacing: 6) {
            ScrollView(.horizontal, showsIndicators: false) {
                Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
                    GridRow {
                        ForEach(columns.indices, id: \.self) { i in
                            // The cockpit's head: mono capitals.
                            Text(columns[i].label)
                                .font(.kouEyebrow)
                                .tracking(0.8)
                                .textCase(.uppercase)
                                .foregroundStyle(Color.kouFg3)
                                .lineLimit(1)
                                .gridColumnAlignment(SurfaceKinds.align(columns[i].align))
                        }
                    }
                    Hairline()
                    ForEach(shown.indices, id: \.self) { r in
                        GridRow {
                            ForEach(columns.indices, id: \.self) { i in
                                let value = cell(shown[r], columns[i], i)
                                Text(Self.text(value))
                                    .font(.kouMeta)
                                    .monospacedDigit()
                                    .foregroundStyle(value == nil || value == .null ? Color.kouFg4 : Color.kouFg)
                                    .lineLimit(1)
                                    .frame(maxWidth: 260, alignment: Alignment(horizontal: SurfaceKinds.align(columns[i].align), vertical: .center))
                            }
                        }
                    }
                }
                .padding(.vertical, 2)
            }
            if more > 0 {
                Text("\(SurfaceText.num(Double(more))) more \(more == 1 ? "row" : "rows") \(reverse ? "before" : "after") these")
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg3)
            }
        }
    }

    private func columns(_ rows: [SurfaceValue], _ shown: [SurfaceValue]) -> [Column] {
        var columns: [Column]
        if let spec = c.array("columns"), !spec.isEmpty {
            columns = spec.map { column in
                if let key = column.string { return Column(key: key, label: key) }
                let o = column.object
                let key = o.flatMap { $0["key"] }.map(SurfaceKinds.jsString) ?? ""
                let label = o?.string("label").flatMap { $0.isEmpty ? nil : $0 } ?? key
                return Column(key: key, label: label, align: o?.string("align"))
            }
        } else if let first = rows.first(where: { $0.object != nil })?.object {
            columns = first.keys.prefix(8).map { Column(key: $0, label: $0) }
        } else {
            columns = [Column(key: "value", label: "value")]
        }
        for i in columns.indices where !["left", "right", "center"].contains(columns[i].align ?? "") {
            let values = shown.compactMap { cell($0, columns[i], i) }.filter { !$0.isNull }
            let numbers = !values.isEmpty && values.allSatisfy { $0.number != nil }
            columns[i].align = numbers ? "right" : "left"
        }
        return columns
    }

    static func text(_ value: SurfaceValue?) -> String {
        if case let .number(n)? = value { return SurfaceText.num(n) }
        return SurfaceText.shown(value)
    }
}

struct SurfaceList: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    var body: some View {
        let items = env.resolve(c["items"])
        if let items = items?.array {
            let ordered = c.bool("reverse") == true ? Array(items.reversed()) : items
            let limit = c.int("limit").flatMap { $0 > 0 ? $0 : nil } ?? 10
            let template = c.string("text")
            let more = items.count - min(items.count, limit)
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(ordered.prefix(limit).enumerated()), id: \.offset) { _, item in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        // U+FE0E: the square as text, as the cockpit's marker.
                        Text("▪\u{FE0E}")
                            .font(.footnote)
                            .foregroundStyle(Color.kouFg4)
                        Text(template != nil && item.object != nil ? SurfaceText.fillItem(item, template ?? "") : SurfaceText.shown(item))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .font(.subheadline)
                }
                if more > 0 {
                    Text("\(SurfaceText.num(Double(more))) more")
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg3)
                }
            }
        } else {
            SurfaceNone(text: items == nil ? "Nothing yet" : "Not a list: \(SurfaceText.shown(items))")
        }
    }
}

// MARK: Charts

/// SurfaceChartData is what a Chart shows, as the web cockpit reads it.
struct SurfaceChartData {
    struct Point: Identifiable {
        let id: Int
        let series: String
        let x: Int
        let value: Double
    }

    var fields: [String] = []
    var alt = ""
    var unit = ""
    var kind = "line"
    var xs: [String] = []
    var series: [[Double?]] = []
    var points: [Point] = []
    var empty: String?
    var height: CGFloat = 160

    init(_ env: SurfaceEnv, _ c: SurfaceObject) {
        if let y = c.string("y") {
            fields = [y]
        } else {
            fields = Array((c.array("y") ?? []).compactMap(\.string).prefix(6))
        }
        alt = c.string("alt") ?? ""
        unit = c["unit"].map(SurfaceKinds.jsString) ?? ""
        kind = ["line", "bar", "points"].contains(c.string("kind") ?? "") ? c.string("kind") ?? "line" : "line"
        if let h = c.int("height"), h >= 60 { height = CGFloat(min(h, 600)) }
        let resolved = env.resolve(c["data"])
        guard let all = resolved?.array, !all.isEmpty, !fields.isEmpty else {
            empty = resolved == nil ? "No points yet · \(alt)" : "No points · \(alt)"
            return
        }
        let limit = c.int("limit").flatMap { $0 > 0 ? min($0, 2000) : nil } ?? 2000
        let data = Array(all.suffix(limit))
        let x = c.string("x")
        xs = data.enumerated().map { i, p in
            if let x, let v = p.object?[x], !v.isNull { return SurfaceText.shown(v) }
            return String(i + 1)
        }
        series = fields.map { field in
            data.map { p in
                let v = p.object.map { $0[field] ?? .null } ?? p
                return SurfaceText.asNumber(v)
            }
        }
        for (j, s) in series.enumerated() {
            for (i, v) in s.enumerated() {
                if let v, v.isFinite { points.append(Point(id: points.count, series: fields[j], x: i, value: v)) }
            }
        }
        if points.isEmpty { empty = "No numbers in \(fields.joined(separator: ", ")) · \(alt)" }
    }

    var count: Int { xs.count }

    func value(_ v: Double?) -> String {
        guard let v else { return "—" }
        return SurfaceText.num(v) + (unit.isEmpty ? "" : " \(unit)")
    }

    /// ticks are where the x axis is labelled: the first point, the middle
    /// one and the last.
    var ticks: [Int] {
        let n = count
        var at = [0]
        if n > 2 { at.append((n - 1) / 2) }
        if n > 1 { at.append(n - 1) }
        return at
    }
}

struct SurfaceChart: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    @State private var picked: Int?
    @State private var pickedBar: String?

    var body: some View {
        let data = SurfaceChartData(env, c)
        VStack(alignment: .leading, spacing: 6) {
            if let empty = data.empty {
                SurfaceNone(text: empty)
            } else {
                legend(data)
                plot(data)
                    .frame(height: data.height)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(data.alt)
    }

    private var at: Int? { picked ?? pickedBar.flatMap(Int.init) }

    /// legendValue is the value a series shows in the legend: the picked
    /// point's, or else its last number.
    private static func legendValue(_ values: [Double?], _ at: Int?) -> Double? {
        if let at { return values.indices.contains(at) ? values[at] : nil }
        return values.last(where: { $0 != nil }) ?? nil
    }

    private func legend(_ data: SurfaceChartData) -> some View {
        let colors = SurfaceKinds.series
        let at = self.at.flatMap { data.xs.indices.contains($0) ? $0 : nil }
        return VStack(alignment: .leading, spacing: 4) {
            if !data.alt.isEmpty {
                Text(data.alt)
                    .font(.caption)
                    .foregroundStyle(Color.kouFg2)
            }
            // The cockpit's legend: mono, the values bright.
            SurfaceFlow(spacing: 10, lineSpacing: 4) {
                if let at {
                    Text(data.xs[at])
                        .font(.system(.caption2, design: .monospaced, weight: .semibold))
                        .foregroundStyle(Color.kouFg)
                }
                ForEach(data.fields.indices, id: \.self) { j in
                    let shown = Self.legendValue(data.series[j], at)
                    HStack(spacing: 4) {
                        RoundedRectangle(cornerRadius: .corner(2))
                            .fill(colors[j % colors.count])
                            .frame(width: 8, height: 8)
                        Text(data.fields[j])
                            .foregroundStyle(Color.kouFg3)
                        Text(data.value(shown))
                            .fontWeight(.semibold)
                            .monospacedDigit()
                            .foregroundStyle(Color.kouFg)
                    }
                    .font(.system(.caption2, design: .monospaced))
                }
            }
        }
    }

    @ViewBuilder
    private func plot(_ data: SurfaceChartData) -> some View {
        let colors = Array(SurfaceKinds.series.prefix(data.fields.count))
        switch data.kind {
        case "bar":
            Chart(data.points) { p in
                BarMark(x: .value("x", String(p.x)), y: .value("value", p.value))
                    .foregroundStyle(by: .value("series", p.series))
                    .position(by: .value("series", p.series))
            }
            .chartForegroundStyleScale(domain: data.fields, range: colors)
            .chartLegend(.hidden)
            .chartXSelection(value: $pickedBar)
            .chartXAxis {
                AxisMarks(values: data.ticks.map(String.init)) { value in
                    AxisValueLabel {
                        if let i = value.as(String.self).flatMap(Int.init), data.xs.indices.contains(i) {
                            Text(data.xs[i])
                        }
                    }
                }
            }
        case "points":
            Chart(data.points) { p in
                PointMark(x: .value("x", p.x), y: .value("value", p.value))
                    .foregroundStyle(by: .value("series", p.series))
                    .symbolSize(18)
            }
            .chartForegroundStyleScale(domain: data.fields, range: colors)
            .chartLegend(.hidden)
            .chartYScale(domain: .automatic(includesZero: false))
            .chartXScale(domain: 0...max(data.count - 1, 1))
            .chartXSelection(value: $picked)
            .chartXAxis { xAxis(data) }
        default:
            Chart {
                ForEach(data.points) { p in
                    LineMark(x: .value("x", p.x), y: .value("value", p.value), series: .value("series", p.series))
                        .foregroundStyle(by: .value("series", p.series))
                        .interpolationMethod(.monotone)
                }
                if let picked {
                    RuleMark(x: .value("x", picked))
                        .foregroundStyle(Color.kouFg4)
                }
            }
            .chartForegroundStyleScale(domain: data.fields, range: colors)
            .chartLegend(.hidden)
            .chartYScale(domain: .automatic(includesZero: false))
            .chartXScale(domain: 0...max(data.count - 1, 1))
            .chartXSelection(value: $picked)
            .chartXAxis { xAxis(data) }
        }
    }

    private func xAxis(_ data: SurfaceChartData) -> some AxisContent {
        AxisMarks(values: data.ticks) { value in
            AxisGridLine()
            AxisValueLabel {
                if let i = value.as(Int.self), data.xs.indices.contains(i) {
                    Text(data.xs[i])
                }
            }
        }
    }
}

// MARK: Files

struct SurfaceImage: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    var body: some View {
        let file = c.string("file") ?? ""
        let alt = c.string("alt") ?? ""
        VStack(alignment: .leading, spacing: 4) {
            RemoteImage(client: env.client, path: API.workspace(env.workspace, "raw"),
                        query: [URLQueryItem(name: "path", value: file)], contentMode: .fit)
                .frame(maxWidth: .infinity, maxHeight: 320)
                .clipShape(.rect(cornerRadius: .corner(8)))
                .contentShape(.rect)
                .onTapGesture { env.open(file, nil) }
                .accessibilityLabel(alt)
            if !alt.isEmpty {
                Text(alt)
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg3)
            }
        }
    }
}

/// The box a file's code or diff is shown in, with its name and a way to
/// open it.
struct SurfaceFileBox<Content: View>: View {
    let name: String
    var detail: String? = nil
    var stat: (added: Int, removed: Int)? = nil
    var open: (() -> Void)? = nil
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text(name)
                    .font(.system(.caption, design: .monospaced, weight: .semibold))
                    .foregroundStyle(Color.kouFg2)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if let detail {
                    Text(detail)
                        .font(.caption2)
                        .foregroundStyle(Color.kouFg3)
                        .lineLimit(1)
                }
                if let stat {
                    HStack(spacing: 4) {
                        Text("+\(stat.added)").foregroundStyle(Color.kouOK)
                        Text("−\(stat.removed)").foregroundStyle(Color.kouErr)
                    }
                }
                Spacer(minLength: 0)
                if let open {
                    Button("Open ↗", action: open)
                        .font(.caption)
                        .buttonStyle(.borderless)
                        .tint(Color.kouAccent)
                }
            }
            .font(.caption2.monospacedDigit())
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.kouSurface)
        .clipShape(.rect(cornerRadius: .corner(12)))
        .overlay(RoundedRectangle(cornerRadius: .corner(12)).strokeBorder(Color.kouLine, lineWidth: 1))
    }
}

struct SurfaceCode: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    @State private var all = false

    /// How many lines show until all are asked for.
    static let folded = 30
    /// How many lines show at most.
    static let most = 400

    var body: some View {
        let file = c.string("file") ?? ""
        let lines = c.string("lines")
        let inline = c["text"] != nil || file.isEmpty
        let name = file.isEmpty ? (c.string("language") ?? "code") : file + (lines.map { ":\($0)" } ?? "")
        let range = Self.range(lines)
        SurfaceFileBox(name: name, open: file.isEmpty ? nil : { env.open(file, range.from) }) {
            if inline {
                code(env.text(c["text"]), runs: nil, classes: nil, from: 1, to: 0)
            } else if let got = env.book.files[file] {
                if let error = got.error {
                    SurfaceNone(text: error)
                        .padding(10)
                } else {
                    code(got.text ?? "", runs: got.runs, classes: got.classes, from: range.from, to: range.to)
                }
            } else {
                SurfaceNone(text: "Reading…")
                    .padding(10)
            }
        }
        .task(id: inline ? "" : file) {
            if !inline { await env.book.want(file) }
        }
    }

    /// range is the lines a "10-40" names; 1 and 0 (to the end) without one.
    static func range(_ lines: String?) -> (from: Int, to: Int) {
        let parts = (lines ?? "").trimmingCharacters(in: .whitespaces).split(separator: "-", omittingEmptySubsequences: false)
        guard (1...2).contains(parts.count), let from = Int(parts[0]), from >= 0 else { return (1, 0) }
        if parts.count == 1 { return (from, from) }
        guard let to = Int(parts[1]) else { return (1, 0) }
        return (from, to)
    }

    @ViewBuilder
    private func code(_ text: String, runs: [Int32]?, classes: [String]?, from: Int, to: Int) -> some View {
        let breaks = text.utf8.reduce(0) { $1 == 0x0A ? $0 + 1 : $0 }
        let count = breaks > 0 && text.hasSuffix("\n") ? breaks : breaks + 1
        let first = max(1, min(from == 0 ? 1 : from, count))
        let last = min(count, to > 0 ? to : count, first + Self.most - 1)
        let end = all ? last : min(last, first + Self.folded - 1)
        let colored = ChangeRows.colored(text, runs: runs, classes: classes, limit: end, drop: 0, total: count)
        let shown = Array(colored.dropFirst(first - 1).prefix(end - first + 1))
        let digits = String(end).count
        let hidden = (to <= 0 ? count : last) - end
        VStack(alignment: .leading, spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(shown.indices, id: \.self) { i in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Text(String(first + i))
                                .foregroundStyle(Color.kouFg4)
                                .frame(width: CGFloat(digits) * 8, alignment: .trailing)
                            Text(shown[i])
                        }
                    }
                }
                .font(.system(.caption, design: .monospaced))
                .padding(.horizontal, 10)
                .padding(.bottom, 8)
            }
            if hidden > 0 {
                Button("Show \(SurfaceText.num(Double(hidden))) more \(hidden == 1 ? "line" : "lines")") {
                    withAnimation(.snappy) { all = true }
                }
                .font(.caption.weight(.medium))
                .buttonStyle(.borderless)
                .padding(.horizontal, 10)
                .padding(.bottom, 8)
                .disabled(all)
            }
        }
    }
}

struct SurfaceDiff: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    @State private var all = false

    struct Line {
        enum Kind { case hunk, added, removed, kept }
        let kind: Kind
        let old: Int?
        let new: Int?
        let text: String
    }

    static let folded = 40
    static let most = 400

    var body: some View {
        let id = c.string("id") ?? ""
        let kept: SurfaceObject = if let patch = c.string("patch") {
            SurfaceObject([("patch", .string(patch))])
        } else {
            env.version.diffs.object(id) ?? SurfaceObject([("error", .string("the call kept no diff"))])
        }
        let file = c.string("file") ?? ""
        let raw = kept.string("patch").map { $0.components(separatedBy: "\n") } ?? []
        let added = raw.filter { $0.hasPrefix("+") && !$0.hasPrefix("+++") }.count
        let removed = raw.filter { $0.hasPrefix("-") && !$0.hasPrefix("---") }.count
        SurfaceFileBox(name: file.isEmpty ? "patch" : file,
                       detail: file.isEmpty ? nil : "against \(c.string("against") ?? "HEAD")",
                       stat: (added, removed),
                       open: file.isEmpty ? nil : { env.open(file, nil) }) {
            VStack(alignment: .leading, spacing: 0) {
                if let error = kept.string("error") {
                    SurfaceNone(text: error)
                        .padding(10)
                } else if added == 0 && removed == 0 {
                    SurfaceNone(text: "No changes")
                        .padding(10)
                } else {
                    lines(Self.parse(raw))
                }
                if kept.bool("cut") == true {
                    Text("The diff was cut at 64 KB.")
                        .font(.caption)
                        .foregroundStyle(Color.kouFg3)
                        .padding(10)
                }
            }
        }
    }

    /// parse reads a unified diff's hunks, with the lines' numbers before
    /// and after.
    static func parse(_ raw: [String]) -> [Line] {
        var out: [Line] = []
        var old = 0
        var new = 0
        var inHunk = false
        for line in raw {
            if line.hasPrefix("@@ "), let hunk = head(line) {
                old = hunk.old
                new = hunk.new
                inHunk = true
                out.append(Line(kind: .hunk, old: nil, new: nil, text: line))
                continue
            }
            if !inHunk || line.hasPrefix("\\") { continue }
            let kind: Line.Kind = line.hasPrefix("+") ? .added : line.hasPrefix("-") ? .removed : .kept
            if kind != .kept || line.hasPrefix(" ") {
                out.append(Line(kind: kind, old: kind == .added ? nil : old, new: kind == .removed ? nil : new, text: String(line.dropFirst())))
            }
            if kind != .added { old += 1 }
            if kind != .removed { new += 1 }
        }
        return out
    }

    /// head reads "@@ -12,5 +12,7 @@": where a hunk starts, before and after.
    private static func head(_ line: String) -> (old: Int, new: Int)? {
        let parts = line.split(separator: " ")
        guard parts.count >= 3, parts[1].hasPrefix("-"), parts[2].hasPrefix("+"),
              let old = Int(parts[1].dropFirst().prefix { $0 != "," }),
              let new = Int(parts[2].dropFirst().prefix { $0 != "," }) else { return nil }
        return (old, new)
    }

    @ViewBuilder
    private func lines(_ all: [Line]) -> some View {
        let limit = self.all ? Self.most : Self.folded
        let shown = Array(all.prefix(limit))
        let hidden = all.count - shown.count
        let digits = String(shown.compactMap { max($0.old ?? 0, $0.new ?? 0) }.max() ?? 0).count
        VStack(alignment: .leading, spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(shown.indices, id: \.self) { i in
                        row(shown[i], digits: digits)
                    }
                }
                .font(.system(.caption, design: .monospaced))
                .padding(.bottom, 6)
            }
            if hidden > 0 {
                Button(self.all ? "\(SurfaceText.num(Double(hidden))) more lines" : "Show \(SurfaceText.num(Double(hidden))) more lines") {
                    withAnimation(.snappy) { self.all = true }
                }
                .font(.caption.weight(.medium))
                .buttonStyle(.borderless)
                .disabled(self.all)
                .padding(.horizontal, 10)
                .padding(.bottom, 8)
            }
        }
    }

    private func row(_ line: Line, digits: Int) -> some View {
        let width = CGFloat(max(digits, 2)) * 8
        let sign = switch line.kind {
        case .added: "+"
        case .removed: "−"
        case .kept, .hunk: " "
        }
        return HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(line.old.map(String.init) ?? "")
                .frame(width: width, alignment: .trailing)
                .foregroundStyle(Color.kouFg4)
            Text(line.new.map(String.init) ?? "")
                .frame(width: width, alignment: .trailing)
                .foregroundStyle(Color.kouFg4)
            // The cockpit's diff: what stayed dimmer than what changed.
            Text(line.kind == .hunk ? line.text : sign + " " + line.text)
                .foregroundStyle(line.kind == .hunk ? Color.kouFg3 : line.kind == .kept ? Color.kouFg2 : Color.kouFg)
                .padding(.trailing, 10)
        }
        .padding(.vertical, 1)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Self.ground(line.kind))
    }

    /// A line's band, in the colours of the cockpit's diffs.
    private static func ground(_ kind: Line.Kind) -> Color {
        switch kind {
        case .added: Color(uiColor: CodeInk.added)
        case .removed: Color(uiColor: CodeInk.removed)
        case .hunk: Color(uiColor: CodeInk.hunk)
        case .kept: .clear
        }
    }
}

struct SurfaceFileLink: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    var body: some View {
        let file = c.string("file") ?? ""
        let line = c.int("line")
        let name = (c["label"]?.truthy ?? false) ? env.label(c) : file + (line.map { ":\($0)" } ?? "")
        Button {
            env.open(file, line)
        } label: {
            Label(name, systemImage: "arrow.up.forward.square")
                .lineLimit(1)
        }
        .font(.footnote)
        .buttonStyle(.borderless)
        .tint(Color.kouAccent)
    }
}

// MARK: Inputs

struct SurfaceButton: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    @State private var sending = false
    @State private var copies = 0

    var body: some View {
        let id = c.string("id") ?? ""
        let used = c.bool("once") == true && env.book.isUsed(env.path, id)
        let disabled = !env.live || used || sending || (env.resolve(c["disabled"])?.truthy ?? false)
        let button = Button {
            Task { await act() }
        } label: {
            Text(env.label(c) + (used ? " ✓" : ""))
                .lineLimit(1)
        }
        .controlSize(.small)
        .buttonBorderShape(.cornerCapsule)
        .disabled(disabled)
        .sensoryFeedback(.success, trigger: copies)
        // Bordered, as the transcript's own buttons are: glass is for what
        // floats over it, and lets the card through.
        switch c.string("style") {
        case "primary":
            button
                .buttonStyle(.borderedProminent)
                .tint(Color.kouAccent)
        case "danger":
            button
                .buttonStyle(.bordered)
                .tint(Color.kouErr)
        default:
            button
                .buttonStyle(.bordered)
        }
    }

    /// act does what the button's action says.
    private func act() async {
        guard let action = c.object("action") else { return }
        if let event = action.object("event") {
            sending = true
            _ = await env.book.event(env.call, env.doc, c, event)
            sending = false
        } else if let set = action.object("set") {
            for (pointer, value) in set.pairs {
                env.book.set(env.path, pointer, env.resolve(value) ?? .null)
            }
        } else if let open = action["open"] {
            let file = open.string ?? env.resolve(open)?.string ?? ""
            if !file.isEmpty { env.open(file, nil) }
        } else if let copy = action["copy"] {
            let text: String
            if let template = copy.string {
                text = SurfaceText.fill(env.doc, env.comps, template)
            } else {
                let value = env.resolve(copy)
                text = value?.string ?? SurfaceText.shown(value)
            }
            UIPasteboard.general.string = text
            copies += 1
        }
    }
}

/// SurfaceField is an input with its label over it.
struct SurfaceField<Control: View>: View {
    let label: String
    @ViewBuilder let control: Control

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if !label.isEmpty {
                // The cockpit's field label: mono capitals.
                Eyebrow(label)
            }
            control
        }
    }
}

struct SurfaceTextField: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    var body: some View {
        let pointer = SurfacePointer.binding(c["value"])
        let value = env.resolve(c["value"])
        let current: String = switch value {
        case nil, .null?: ""
        case let .some(v): SurfaceKinds.jsString(v)
        }
        let multiline = c.bool("multiline") == true
        let text = Binding<String>(get: { current }, set: { new in
            if let pointer { env.book.set(env.path, pointer, .string(new)) }
        })
        SurfaceField(label: env.label(c)) {
            TextField(c["placeholder"].map { env.text($0) } ?? "", text: text, axis: multiline ? .vertical : .horizontal)
                .lineLimit(multiline ? 3 : 1, reservesSpace: multiline)
                .font(.subheadline)
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                // The cockpit's input: the screen's ground, cut into the card.
                .background(Color.kouGround, in: .rect(cornerRadius: .corner(8)))
                .overlay(RoundedRectangle(cornerRadius: .corner(8)).strokeBorder(Color.kouLine2, lineWidth: 1))
                .disabled(!env.live || pointer == nil)
        }
    }
}

struct SurfaceToggle: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    var body: some View {
        let pointer = SurfacePointer.binding(c["value"])
        let on = env.resolve(c["value"]) == .bool(true)
        Toggle(isOn: Binding(get: { on }, set: { new in
            if let pointer { env.book.set(env.path, pointer, .bool(new)) }
        })) {
            Text(env.label(c))
                .font(.subheadline)
        }
        .tint(Color.kouAccent)
        .disabled(!env.live || pointer == nil)
    }
}

struct SurfaceSelect: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    var body: some View {
        let pointer = SurfacePointer.binding(c["value"])
        let options: [(value: SurfaceValue, label: String)] = (c.array("options") ?? []).map { option in
            if let o = option.object {
                let label = o["label"].flatMap { $0.isNull ? nil : $0.string ?? SurfaceKinds.jsString($0) }
                    ?? SurfaceText.shown(o["value"])
                return (o["value"] ?? .null, label)
            }
            return (option, SurfaceText.shown(option))
        }
        let current = env.resolve(c["value"])
        let at = options.firstIndex { option in
            guard let current else { return false }
            if option.value == current { return true }
            return !current.isNull && SurfaceKinds.jsString(option.value) == SurfaceKinds.jsString(current)
        }
        HStack {
            Text(env.label(c))
                .font(.subheadline)
            Spacer(minLength: 8)
            Picker(env.label(c), selection: Binding(get: { at ?? -1 }, set: { i in
                if let pointer, options.indices.contains(i) { env.book.set(env.path, pointer, options[i].value) }
            })) {
                if at == nil {
                    Text("—").tag(-1)
                }
                ForEach(options.indices, id: \.self) { i in
                    Text(options[i].label).tag(i)
                }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .tint(Color.kouAccent)
            .disabled(!env.live || pointer == nil)
        }
    }
}

struct SurfaceSlider: View {
    let env: SurfaceEnv
    let c: SurfaceObject

    var body: some View {
        let pointer = SurfacePointer.binding(c["value"])
        let low = c["min"].flatMap(SurfaceText.asNumber) ?? 0
        let high = c["max"].flatMap(SurfaceText.asNumber) ?? 100
        let step = c["step"].flatMap(SurfaceText.asNumber).flatMap { $0 > 0 ? $0 : nil } ?? 1
        let value = env.resolve(c["value"]).flatMap(SurfaceText.asNumber)
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline) {
                Eyebrow(env.label(c))
                Spacer()
                Text(value.map { SurfaceText.num($0) } ?? "—")
                    .font(.system(.caption, design: .monospaced, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(value == nil ? Color.kouFg4 : Color.kouFg)
            }
            if high > low {
                Slider(value: Binding(get: { min(max(value ?? low, low), high) }, set: { new in
                    // As the step has it, without a float's tail.
                    let snapped = ((new - low) / step).rounded() * step + low
                    if let pointer { env.book.set(env.path, pointer, .number((snapped * 1e9).rounded() / 1e9)) }
                }), in: low...high)
                .tint(Color.kouAccent)
                .disabled(!env.live || pointer == nil)
            }
        }
    }
}
