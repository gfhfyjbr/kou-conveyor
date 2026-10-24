import SwiftUI

/// NodeList is the canvas's nodes, for a screen too small for its board:
/// those an agent proposed first.
struct NodeList: View {
    let model: CanvasModel
    var act: (NodeAction) -> Void

    var body: some View {
        let proposed = model.proposed
        let placed = model.nodes.filter { !$0.isProposed }
        List {
            if !proposed.isEmpty {
                Section {
                    ForEach(proposed) { row($0) }
                    FormNote("Nothing runs for them until you approve them.")
                        .listNote()
                } header: {
                    SectionLabel("Proposed by agents", count: proposed.count, color: .kouWarn)
                        .kouHeader()
                }
            }
            if !placed.isEmpty {
                Section {
                    ForEach(placed) { row($0) }
                } header: {
                    SectionLabel("Nodes", count: placed.count)
                        .kouHeader(top: proposed.isEmpty ? 14 : 22)
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color.kouGround)
        .animation(.snappy, value: model.nodes.map { "\($0.id)\($0.isProposed)" })
        .overlay {
            if model.nodes.isEmpty {
                KouEmpty(symbol: "square.dashed", title: "No nodes",
                         message: "Add an agent, a terminal, a source or a note with +, or ask the foreman to build it.")
            }
        }
    }

    private func row(_ node: CanvasNode) -> some View {
        Button {
            act(.open(node))
        } label: {
            NodeRow(model: model, node: node)
        }
        .listRow()
        .contextMenu { NodeMenu(model: model, node: node, act: act) }
        .swipeActions(edge: .trailing) {
            if !model.readOnly {
                Button("Remove", systemImage: "trash") { act(.remove(node)) }
                    .tint(Color.kouErr)
            }
        }
        .swipeActions(edge: .leading) {
            if node.isProposed, !model.readOnly {
                Button("Approve", systemImage: "checkmark") { act(.approve(node)) }
                    .tint(Color.kouAccent)
            } else {
                Button("Details", systemImage: "info.circle") { act(.inspect(node)) }
                    .tint(Color.kouFg3)
            }
        }
    }
}

/// NodeRow is a node in a list: its kind's tile, what it is and does, what
/// waits for it, its state.
struct NodeRow: View {
    let model: CanvasModel
    let node: CanvasNode

    var body: some View {
        let status = model.status[node.id]
        let state = node.isProposed ? "proposed" : status?.state
        let awaiting = model.awaiting(for: node.id)
        let queued = max(model.pending(for: node.id) - awaiting, 0)
        HStack(spacing: 12) {
            NodeTile(node: node, state: state)
            VStack(alignment: .leading, spacing: 3) {
                Text(node.title.isEmpty ? node.kind.capitalized : node.title)
                    .font(.body.weight(.medium))
                    .foregroundStyle(Color.kouFg)
                    .lineLimit(1)
                Text(line(status))
                    .font(.kouMeta)
                    .foregroundStyle(state == "busy" ? Color.kouAccent : Color.kouFg3)
                    .lineLimit(1)
            }
            Spacer(minLength: 6)
            if awaiting > 0 {
                CountBadge(count: awaiting, symbol: "hand.raised.fill", tint: .kouWarn)
            }
            if queued > 0 {
                CountBadge(count: queued, symbol: "tray.full.fill", tint: .kouFg2)
            }
            if node.isProposed {
                StateBadge(state: "proposed")
            } else if let status, node.kind != CanvasNode.note {
                // A note runs nothing: its state says nothing.
                StateBadge(state: status.state)
            }
        }
        .contentShape(.rect)
    }

    private func line(_ status: NodeStatus?) -> String {
        if node.isProposed {
            return "Proposed by \(NodeInspector.creator(of: node, in: model))"
        }
        if let activity = status?.activity, !activity.isEmpty { return activity }
        if let detail = status?.detail, !detail.isEmpty { return detail }
        return NodeInspector.kindName(of: node, in: model)
    }
}

/// NodeTile is a node's mark in a list: its kind's glyph on a tile, as the
/// canvases' own — lit, with the light going round, while it works; dashed
/// in the warning's while an agent only proposed it.
struct NodeTile: View {
    let node: CanvasNode
    var state: String?
    var size: CGFloat = 34

    private var working: Bool { state == "busy" || state == "starting" }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: .corner(size * 0.26), style: .continuous)
        let note = node.kind == CanvasNode.note
        let tone: Color = note || node.isProposed ? .kouWarn : node.kind == CanvasNode.agent || working ? .kouAccent : .kouFg2
        Image(systemName: NodeKindIcon.symbol(node.kind))
            .font(.system(size: size * 0.42, weight: .medium))
            .foregroundStyle(tone)
            .frame(width: size, height: size)
            .background(working ? Color.kouAccentSoft : Color.kouPanel, in: shape)
            .overlay {
                shape.strokeBorder(node.isProposed ? Color.kouWarn.opacity(0.6) : working ? Color.kouAccent.opacity(0.35) : Color.kouLine,
                                   style: StrokeStyle(lineWidth: 1, dash: node.isProposed ? [3, 2.5] : []))
            }
            .runLight(state == "busy", in: shape)
            .accessibilityHidden(true)
    }
}

/// CountBadge is a count of what waits, as the cockpit boxes one: mono
/// figures in a hairline of their tone.
struct CountBadge: View {
    let count: Int
    let symbol: String
    let tint: Color
    var size: CGFloat = 11

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: .corner(4), style: .continuous)
        HStack(spacing: 3) {
            Image(systemName: symbol)
                .font(.system(size: size * 0.75, weight: .bold))
            Text(verbatim: "\(count)")
                .monospacedDigit()
        }
        .font(.system(size: size, weight: .bold, design: .monospaced))
        .foregroundStyle(tint)
        .padding(.horizontal, 5)
        .padding(.vertical, 2)
        .background(tint.opacity(0.1), in: shape)
        .overlay(shape.strokeBorder(tint.opacity(0.4), lineWidth: 1))
        .fixedSize()
        .accessibilityElement(children: .combine)
    }
}

/// NodeMenu is what can be done to a node, held down on the board or in a
/// list.
struct NodeMenu: View {
    let model: CanvasModel
    let node: CanvasNode
    var act: (NodeAction) -> Void

    var body: some View {
        if node.isProposed, !model.readOnly {
            Button("Approve", systemImage: "checkmark.circle") { act(.approve(node)) }
            Button("Discard", systemImage: "xmark.circle", role: .destructive) { act(.discard(node)) }
            Divider()
        }
        if NodeChat.route(for: node, in: model) != nil {
            Button("Open the chat", systemImage: "bubble.left.and.bubble.right") { act(.open(node)) }
        }
        Button("Details", systemImage: "info.circle") { act(.inspect(node)) }
        if !model.readOnly {
            Button("Rename…", systemImage: "pencil") { act(.rename(node)) }
            if !node.isProposed {
                Divider()
                Button("Remove", systemImage: "trash", role: .destructive) { act(.remove(node)) }
            }
        }
    }
}

/// CanvasInbox is what the canvas has for the user: the nodes agents
/// proposed and the messages that wait for approval, first; then those
/// that wait for their node, and the latest.
struct CanvasInbox: View {
    let model: CanvasModel
    var act: (NodeAction) -> Void
    var read: (CanvasMessage) -> Void

    var body: some View {
        let queued = model.queue.filter { !$0.awaitsApproval }
        let approving = model.awaitingApproval
        // The first header sits higher than the next ones.
        let first = !model.proposed.isEmpty ? 0 : !approving.isEmpty ? 1 : !queued.isEmpty ? 2 : 3
        List {
            if !model.proposed.isEmpty {
                Section {
                    ForEach(model.proposed) { node in
                        ProposedRow(model: model, node: node, act: act)
                            .listRow()
                    }
                } header: {
                    SectionLabel("Nodes proposed", count: model.proposed.count, color: .kouWarn)
                        .kouHeader()
                }
            }
            if !approving.isEmpty {
                Section {
                    ForEach(approving) { message in
                        row(message)
                            .swipeActions(edge: .leading) {
                                Button("Send", systemImage: "paperplane.fill") {
                                    Task { await model.approve(message) }
                                }
                                .tint(Color.kouAccent)
                            }
                    }
                    FormNote("Their wires ask before each message goes. Swipe right to send one as it is; tap it to read, edit or drop it.")
                        .listNote()
                } header: {
                    SectionLabel("Wait for your approval", count: approving.count, color: .kouWarn)
                        .kouHeader(top: first == 1 ? 14 : 22)
                }
            }
            if !queued.isEmpty {
                Section {
                    ForEach(queued) { row($0) }
                    FormNote("They go once their node is free\(model.isLive ? "" : " and the canvas is live").")
                        .listNote()
                } header: {
                    SectionLabel("Queued", count: queued.count)
                        .kouHeader(top: first == 2 ? 14 : 22)
                }
            }
            // No header over nothing: before the first message, the list
            // has only what waits, or the overlay says there is nothing.
            if !model.messages.isEmpty {
                Section {
                    ForEach(model.messages.reversed()) { row($0) }
                } header: {
                    SectionLabel("Latest", count: model.messages.count)
                        .kouHeader(top: first == 3 ? 14 : 22)
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color.kouGround)
        .overlay {
            if model.messages.isEmpty, model.queue.isEmpty, model.proposed.isEmpty {
                KouEmpty(symbol: "arrow.left.arrow.right", title: "No messages yet",
                         message: "What the nodes send each other along their wires shows here, and what waits for you.")
            }
        }
    }

    private func row(_ message: CanvasMessage) -> some View {
        Button {
            read(message)
        } label: {
            MessageRow(model: model, message: message)
        }
        .listRow()
        .swipeActions(edge: .trailing) {
            if message.isWaiting, !model.readOnly {
                Button("Drop", systemImage: "trash") {
                    Task { await model.drop(message) }
                }
                .tint(Color.kouErr)
            }
        }
    }
}

/// ProposedRow is a node an agent proposed, with what the user does about
/// it.
struct ProposedRow: View {
    let model: CanvasModel
    let node: CanvasNode
    var act: (NodeAction) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button {
                act(.inspect(node))
            } label: {
                HStack(spacing: 12) {
                    NodeTile(node: node, state: "proposed")
                    VStack(alignment: .leading, spacing: 3) {
                        Text(node.title)
                            .font(.body.weight(.medium))
                            .foregroundStyle(Color.kouFg)
                            .lineLimit(1)
                        Text("\(NodeInspector.kindName(of: node, in: model)) · by \(NodeInspector.creator(of: node, in: model))")
                            .font(.kouMeta)
                            .foregroundStyle(Color.kouFg3)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 6)
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(Color.kouFg4)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if !model.readOnly {
                HStack(spacing: 8) {
                    // In a list a label's icon takes the tint: on the tinted
                    // button it would not show.
                    Button { act(.approve(node)) } label: {
                        Label("Approve", systemImage: "checkmark")
                            .foregroundStyle(Color.kouAccentInk)
                    }
                    .buttonStyle(.glassProminent)
                    .tint(Color.kouAccent)
                    Button("Discard", systemImage: "xmark", role: .destructive) { act(.discard(node)) }
                        .buttonStyle(.glass)
                        .tint(Color.kouErr)
                }
                .font(.subheadline.weight(.semibold))
                .controlSize(.small)
                // Under its words, past its tile.
                .padding(.leading, 46)
            }
        }
        .padding(.vertical, 2)
    }
}

/// MessageRow is a message between nodes, as the cockpit shows one: who to
/// whom in mono, what it says, and how it stands while it is not through;
/// a bar of the warning's along it while it waits for the user.
struct MessageRow: View {
    let model: CanvasModel
    let message: CanvasMessage

    var body: some View {
        let waits = message.state == CanvasMessage.awaiting
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(name(message.from?.node))
                    .fontWeight(.semibold)
                    .foregroundStyle(Color.kouFg)
                    .layoutPriority(1)
                Image(systemName: "arrow.right")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(waits ? Color.kouWarn : Color.kouFg4)
                Text(name(message.to?.node))
                    .fontWeight(.semibold)
                    .foregroundStyle(Color.kouFg)
                Spacer(minLength: 6)
                if let at = message.at {
                    Text(at, format: .dateTime.hour().minute().second())
                        .monospacedDigit()
                        .foregroundStyle(Color.kouFg4)
                }
            }
            .font(.kouMeta)
            .lineLimit(1)
            if let title = message.title, !title.isEmpty {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Color.kouFg)
                    .lineLimit(2)
            }
            Text(message.content)
                .font(.system(.footnote, design: .monospaced))
                .foregroundStyle(Color.kouFg2)
                .lineLimit(6)
            if let state = message.state, state != CanvasMessage.delivered {
                HStack(spacing: 6) {
                    Rectangle()
                        .fill(Self.color(state))
                        .frame(width: 6, height: 6)
                    Text(Self.describe(message))
                        .font(.kouMeta)
                        .foregroundStyle(Self.color(state))
                        .lineLimit(2)
                }
            }
        }
        .padding(.leading, waits ? 12 : 0)
        .overlay(alignment: .leading) {
            if waits {
                Rectangle()
                    .fill(Color.kouWarn)
                    .frame(width: 2)
            }
        }
        .opacity(message.state == CanvasMessage.dropped ? 0.6 : 1)
        .padding(.vertical, 2)
        .contentShape(.rect)
    }

    private func name(_ node: String?) -> String {
        guard let node, !node.isEmpty else { return "You" }
        return model.node(node)?.title ?? node
    }

    static func describe(_ message: CanvasMessage) -> String {
        let state: String
        switch message.state {
        case CanvasMessage.awaiting: state = "waits for your approval"
        case CanvasMessage.pending: state = "queued"
        case CanvasMessage.dropped: state = "dropped"
        case CanvasMessage.delivered: state = "delivered"
        case let other?: state = other
        case nil: state = ""
        }
        guard let reason = message.reason, !reason.isEmpty else { return state }
        return "\(state): \(reason)"
    }

    /// A message's state's colour: the warning's while it waits for the
    /// user, the error's once dropped, the quiet inks for the rest.
    static func color(_ state: String) -> Color {
        switch state {
        case CanvasMessage.awaiting: .kouWarn
        case CanvasMessage.pending: .kouFg2
        case CanvasMessage.dropped: .kouErr
        default: .kouFg3
        }
    }
}

/// CanvasLog is what happened on the canvas while it was open: what it
/// said, what its agents changed on it — as a log reads, its times aside.
struct CanvasLog: View {
    let model: CanvasModel

    var body: some View {
        List {
            if !model.activity.isEmpty {
                Section {
                    ForEach(model.activity.reversed()) { item in
                        line(item)
                            .listRowBackground(Color.kouGround)
                            .listRowSeparatorTint(Color.kouLine)
                            .listRowInsets(EdgeInsets(top: 9, leading: 16, bottom: 9, trailing: 16))
                    }
                } header: {
                    SectionLabel("While open", count: model.activity.count)
                        .kouHeader()
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color.kouGround)
        .overlay {
            if model.activity.isEmpty {
                KouEmpty(symbol: "list.bullet.rectangle", title: "Nothing yet",
                         message: "What the canvas says, and what its agents change on it, shows here while it is open.")
            }
        }
    }

    private func line(_ item: CanvasActivity) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(item.at, format: .dateTime.hour().minute().second())
                .font(.kouMeta)
                .monospacedDigit()
                .foregroundStyle(Color.kouFg4)
            Image(systemName: symbol(item))
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(color(item))
                .frame(width: 14)
            Text(item.text)
                .font(.subheadline)
                .foregroundStyle(item.level == "error" ? Color.kouErr : Color.kouFg)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func symbol(_ item: CanvasActivity) -> String {
        switch (item.kind, item.level) {
        case (.change, _): "sparkles"
        case (.notice, "error"): "exclamationmark.octagon.fill"
        case (.notice, "warning"): "exclamationmark.triangle.fill"
        default: "info.circle"
        }
    }

    private func color(_ item: CanvasActivity) -> Color {
        switch (item.kind, item.level) {
        case (.change, _): .kouAccent
        case (.notice, "error"): .kouErr
        case (.notice, "warning"): .kouWarn
        default: .kouFg3
        }
    }
}

private extension View {
    /// A row of the canvas's plain lists: on the screen's ground, over its
    /// hairline.
    func listRow() -> some View {
        listRowBackground(Color.kouGround)
            .listRowSeparatorTint(Color.kouLine)
            .listRowInsets(EdgeInsets(top: 11, leading: 16, bottom: 11, trailing: 16))
    }

    /// A word under a plain list's rows: no line under it, nor a ground.
    func listNote() -> some View {
        listRowBackground(Color.kouGround)
            .listRowSeparator(.hidden)
            .listRowInsets(EdgeInsets(top: 2, leading: 16, bottom: 12, trailing: 16))
    }
}
