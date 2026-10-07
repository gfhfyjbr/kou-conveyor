import SwiftUI

/// NodeSheet is a node up close: an agent's feed and a message to it, a
/// terminal's screen and keys to press, a note's text.
struct NodeSheet: View {
    let model: CanvasModel
    let nodeID: String
    @Environment(\.dismiss) private var dismiss
    @State private var draft = ""
    @State private var screen: String?
    @State private var entries: [FeedEntry] = []
    @State private var error: String?
    @State private var sending = false
    @FocusState private var focused: Bool

    private var node: CanvasNode? { model.node(nodeID) }
    private var status: NodeStatus? { model.status[nodeID] }
    private var kind: String { node?.kind ?? "" }
    private var writable: Bool { !model.readOnly && (kind == CanvasNode.agent || kind == CanvasNode.terminal) }

    var body: some View {
        NavigationStack {
            Group {
                switch kind {
                case CanvasNode.agent: feed
                case CanvasNode.terminal, CanvasNode.source: terminal
                case CanvasNode.note: note
                default: ContentUnavailableView("Gone", systemImage: "questionmark.square.dashed", description: Text("The node is no longer on the canvas."))
                }
            }
            .safeAreaBar(edge: .top) {
                if let error {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(.red)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .glassEffect(.regular, in: .capsule)
                        .onTapGesture { self.error = nil }
                }
            }
            .safeAreaBar(edge: .bottom) {
                if writable { input }
            }
            .navigationTitle(node?.title ?? "Node")
            .navigationBarTitleDisplayMode(.inline)
            .navigationSubtitle(subtitle)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
                if !model.readOnly, kind == CanvasNode.agent || kind == CanvasNode.terminal || kind == CanvasNode.source {
                    ToolbarItem(placement: .topBarTrailing) {
                        Menu("Node", systemImage: "ellipsis") {
                            Button("Restart", systemImage: "arrow.clockwise") { act { try await model.restart(nodeID) } }
                            Button("Stop", systemImage: "stop.fill", role: .destructive) { act { try await model.stop(nodeID) } }
                        }
                    }
                }
            }
        }
        .task(id: model.touched[nodeID] ?? 0) { await reload() }
        .task(id: kind) {
            // A terminal's screen changes without telling: look again often.
            guard kind == CanvasNode.terminal || kind == CanvasNode.source else { return }
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(1500))
                await reload()
            }
        }
    }

    private var subtitle: String {
        guard let status else { return kind.capitalized }
        if let activity = status.activity, !activity.isEmpty { return "\(status.state) · \(activity)" }
        return status.state
    }

    // MARK: Kinds

    private var feed: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 12) {
                ForEach(merged) { entry in
                    FeedRow(entry: entry)
                        .id(entry.id)
                }
                if status?.state == "busy" {
                    ActivityRow(activity: status?.activity, started: status?.since, reconnecting: model.reconnecting)
                }
            }
            .padding(16)
        }
        .defaultScrollAnchor(.bottom)
        .defaultScrollAnchor(.bottom, for: .sizeChanges)
        .overlay {
            if merged.isEmpty {
                ContentUnavailableView("Nothing yet", systemImage: "sparkle", description: Text("What the agent does shows here."))
            }
        }
    }

    /// The feed as last read, and what the stream brought since.
    private var merged: [FeedEntry] {
        var list = entries
        var seen = Dictionary(list.enumerated().map { ($1.id, $0) }, uniquingKeysWith: { _, last in last })
        for entry in model.feeds[nodeID] ?? [] {
            if let at = seen[entry.id] {
                list[at] = entry
            } else {
                seen[entry.id] = list.count
                list.append(entry)
            }
        }
        return list
    }

    private var terminal: some View {
        ScrollView([.vertical, .horizontal]) {
            Text(screen ?? "")
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(.green.mix(with: .white, by: 0.35))
                .textSelection(.enabled)
                .fixedSize()
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .defaultScrollAnchor(.bottomLeading)
        .background(Color.black)
        .overlay {
            if screen == nil { ProgressView().tint(.white) }
        }
    }

    private var note: some View {
        ScrollView {
            MarkdownText(text: node?.config?["text"]?.string ?? "")
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    // MARK: Writing

    private var input: some View {
        VStack(spacing: 8) {
            if kind == CanvasNode.terminal {
                ScrollView(.horizontal, showsIndicators: false) {
                    GlassEffectContainer(spacing: 6) {
                        HStack(spacing: 6) {
                            key("⌃C", "C-c")
                            key("esc", "escape")
                            key("tab", "tab")
                            key("↑", "up")
                            key("↓", "down")
                            key("⏎", "enter")
                            key("⌃D", "C-d")
                        }
                        .padding(.horizontal, 12)
                    }
                }
            }
            GlassEffectContainer(spacing: 10) {
                HStack(alignment: .bottom, spacing: 10) {
                    TextField(kind == CanvasNode.terminal ? "Type a command" : "Message the agent", text: $draft, axis: .vertical)
                        .lineLimit(1...5)
                        .font(kind == CanvasNode.terminal ? .system(.body, design: .monospaced) : .body)
                        .textInputAutocapitalization(kind == CanvasNode.terminal ? .never : .sentences)
                        .autocorrectionDisabled(kind == CanvasNode.terminal)
                        .focused($focused)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 11)
                        .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 22))
                    Button {
                        send()
                    } label: {
                        Image(systemName: "arrow.up")
                            .font(.body.weight(.bold))
                            .frame(width: 26, height: 30)
                    }
                    .buttonStyle(.glassProminent)
                    .buttonBorderShape(.circle)
                    .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || sending)
                }
                .padding(.horizontal, 12)
            }
        }
        .padding(.bottom, 8)
    }

    private func key(_ label: String, _ name: String) -> some View {
        Button(label) {
            act { try await model.send(to: nodeID, text: nil, keys: [name]) }
        }
        .font(.system(.footnote, design: .monospaced, weight: .semibold))
        .buttonStyle(.glass)
        .controlSize(.small)
    }

    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        draft = ""
        sending = true
        let submit: Bool? = kind == CanvasNode.terminal ? true : nil
        Task {
            defer { sending = false }
            do {
                try await model.send(to: nodeID, text: text, submit: submit)
                error = nil
                await reload()
            } catch {
                if draft.isEmpty { draft = text }
                self.error = error.localizedDescription
            }
        }
    }

    private func act(_ work: @escaping @MainActor () async throws -> Void) {
        Task {
            do {
                try await work()
                error = nil
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    private func reload() async {
        do {
            switch kind {
            case CanvasNode.agent:
                entries = try await model.transcript(nodeID).entries
            case CanvasNode.terminal:
                screen = try await model.read(nodeID, what: "screen").text
            case CanvasNode.source:
                screen = try await model.read(nodeID, what: "tail", lines: 400).text
            default:
                return
            }
        } catch is CancellationError {
        } catch RelayError.http(409, _, _) {
            // Not running: nothing to read.
            if screen == nil { screen = "" }
        } catch {
            self.error = error.localizedDescription
        }
    }
}

/// FeedRow is a line of an agent node's feed.
struct FeedRow: View {
    let entry: FeedEntry

    var body: some View {
        switch entry.kind {
        case Entry.user:
            Text(entry.text ?? "")
                .foregroundStyle(.white)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(Color.kouAccent.gradient, in: .rect(cornerRadius: 16))
                .frame(maxWidth: .infinity, alignment: .trailing)
                .padding(.leading, 32)
        case Entry.tool:
            HStack(spacing: 8) {
                ToolStateIcon(tool: entry.tool.map { Tool(callID: entry.id, name: $0.name, state: $0.state, exitCode: $0.exitCode) }, live: true)
                    .frame(width: 18)
                Text(entry.tool?.name ?? "tool")
                    .font(.system(.footnote, design: .monospaced, weight: .semibold))
                Text(entry.tool?.summary ?? "")
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        case Entry.reasoning:
            Label(entry.text ?? "Thinking", systemImage: "brain")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .lineLimit(3)
        case Entry.error:
            ErrorBlock(text: entry.text ?? "", detail: nil)
        case Entry.notice:
            NoticeRow(text: entry.text ?? "", detail: nil)
        default:
            MarkdownText(text: entry.text ?? "")
        }
    }
}
