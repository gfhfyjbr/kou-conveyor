import SwiftUI

/// NodeList is the canvas's nodes, for a screen too small for its board:
/// those an agent proposed first.
struct NodeList: View {
    let model: CanvasModel
    var act: (NodeAction) -> Void

    var body: some View {
        List {
            if !model.proposed.isEmpty {
                Section {
                    ForEach(model.proposed) { row($0) }
                } header: {
                    Text("Proposed by agents")
                } footer: {
                    Text("Nothing runs for them until you approve them.")
                }
            }
            Section {
                ForEach(model.nodes.filter { !$0.isProposed }) { row($0) }
            }
        }
        .groupedBackground()
        .overlay {
            if model.nodes.isEmpty {
                ContentUnavailableView("No nodes", systemImage: "square.dashed",
                                       description: Text("Add an agent, a terminal, a source or a note with +, or ask the foreman to build it."))
            }
        }
    }

    private func row(_ node: CanvasNode) -> some View {
        Button {
            act(.open(node))
        } label: {
            NodeRow(model: model, node: node)
        }
        .contextMenu { NodeMenu(model: model, node: node, act: act) }
        .swipeActions(edge: .trailing) {
            if !model.readOnly {
                Button("Remove", systemImage: "trash") { act(.remove(node)) }
                    .tint(.red)
            }
        }
        .swipeActions(edge: .leading) {
            if node.isProposed, !model.readOnly {
                Button("Approve", systemImage: "checkmark") { act(.approve(node)) }
                    .tint(.green)
            } else {
                Button("Details", systemImage: "info.circle") { act(.inspect(node)) }
                    .tint(.blue)
            }
        }
    }
}

/// NodeRow is a node in a list: what it is, what it does, what waits for
/// it.
struct NodeRow: View {
    let model: CanvasModel
    let node: CanvasNode

    var body: some View {
        let status = model.status[node.id]
        let awaiting = model.awaiting(for: node.id)
        let queued = max(model.pending(for: node.id) - awaiting, 0)
        HStack(spacing: 12) {
            NodeKindIcon(kind: node.kind)
                .frame(width: 28)
            VStack(alignment: .leading, spacing: 3) {
                // Color.primary, not .primary: in a button's label .primary
                // is the button's tint.
                Text(node.title.isEmpty ? node.kind.capitalized : node.title)
                    .font(.body.weight(.medium))
                    .foregroundStyle(Color.primary)
                    .lineLimit(1)
                Text(line(status))
                    .font(.caption)
                    .foregroundStyle(Color.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 6)
            if awaiting > 0 {
                CountBadge(count: awaiting, symbol: "hand.raised.fill", tint: .yellow)
            }
            if queued > 0 {
                CountBadge(count: queued, symbol: "tray.full.fill", tint: .kouAccent)
            }
            if node.isProposed {
                StateBadge(state: "proposed")
            } else if let status, node.kind != CanvasNode.note {
                // A note runs nothing: its state says nothing.
                StateBadge(state: status.state)
            }
        }
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

/// CountBadge is a count of what waits.
struct CountBadge: View {
    let count: Int
    let symbol: String
    let tint: Color

    var body: some View {
        Label("\(count)", systemImage: symbol)
            .labelStyle(.titleAndIcon)
            .font(.system(size: 11, weight: .bold, design: .rounded))
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .foregroundStyle(tint)
            .background(tint.opacity(0.16), in: .cornerCapsule)
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
        List {
            if !model.proposed.isEmpty {
                Section("Nodes proposed") {
                    ForEach(model.proposed) { node in
                        ProposedRow(model: model, node: node, act: act)
                    }
                }
            }
            if !model.awaitingApproval.isEmpty {
                Section {
                    ForEach(model.awaitingApproval) { message in
                        row(message)
                            .swipeActions(edge: .leading) {
                                Button("Send", systemImage: "paperplane.fill") {
                                    Task { await model.approve(message) }
                                }
                                .tint(.green)
                            }
                    }
                } header: {
                    Text("Wait for your approval")
                } footer: {
                    Text("Their wires ask before each message goes. Swipe right to send one as it is; tap it to read, edit or drop it.")
                }
            }
            if !queued.isEmpty {
                Section {
                    ForEach(queued) { row($0) }
                } header: {
                    Text("Queued")
                } footer: {
                    Text("They go once their node is free\(model.isLive ? "" : " and the canvas is live").")
                }
            }
            // No header over nothing: before the first message, the list
            // has only what waits, or the overlay says there is nothing.
            if !model.messages.isEmpty {
                Section("Latest") {
                    ForEach(model.messages.reversed()) { row($0) }
                }
            }
        }
        .groupedBackground()
        .overlay {
            if model.messages.isEmpty, model.queue.isEmpty, model.proposed.isEmpty {
                ContentUnavailableView("No messages yet", systemImage: "arrow.left.arrow.right",
                                       description: Text("What the nodes send each other along their wires shows here, and what waits for you."))
            }
        }
    }

    private func row(_ message: CanvasMessage) -> some View {
        Button {
            read(message)
        } label: {
            MessageRow(model: model, message: message)
        }
        .swipeActions(edge: .trailing) {
            if message.isWaiting, !model.readOnly {
                Button("Drop", systemImage: "trash") {
                    Task { await model.drop(message) }
                }
                .tint(.red)
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
                    NodeKindIcon(kind: node.kind)
                        .frame(width: 28)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(node.title)
                            .font(.body.weight(.medium))
                            .foregroundStyle(Color.primary)
                        Text("\(NodeInspector.kindName(of: node, in: model)) · by \(NodeInspector.creator(of: node, in: model))")
                            .font(.caption)
                            .foregroundStyle(Color.secondary)
                    }
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(Color.secondary)
                }
            }
            .buttonStyle(.plain)
            if !model.readOnly {
                HStack(spacing: 10) {
                    // In a list a label's icon takes the tint: on the tinted
                    // button it would not show.
                    Button { act(.approve(node)) } label: {
                        Label("Approve", systemImage: "checkmark")
                            .foregroundStyle(Color.white)
                    }
                    .buttonStyle(.glassProminent)
                    Button("Discard", systemImage: "xmark", role: .destructive) { act(.discard(node)) }
                        .buttonStyle(.glass)
                }
                .controlSize(.small)
            }
        }
        .padding(.vertical, 2)
    }
}

/// MessageRow is a message between nodes.
struct MessageRow: View {
    let model: CanvasModel
    let message: CanvasMessage

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Text(name(message.from?.node))
                Image(systemName: "arrow.right")
                Text(name(message.to?.node))
                Spacer()
                if let at = message.at {
                    Text(at, format: .dateTime.hour().minute().second())
                }
            }
            .font(.caption.weight(.semibold))
            .foregroundStyle(Color.secondary)
            .lineLimit(1)
            if let title = message.title, !title.isEmpty {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Color.primary)
                    .lineLimit(2)
            }
            Text(message.content)
                .font(.subheadline)
                .foregroundStyle(Color.primary)
                .lineLimit(6)
            if let state = message.state, state != CanvasMessage.delivered {
                Text(Self.describe(message))
                    .font(.caption.weight(.medium))
                    .foregroundStyle(Self.color(state))
            }
        }
        .padding(.vertical, 2)
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

    static func color(_ state: String) -> Color {
        switch state {
        case CanvasMessage.awaiting: .yellow
        case CanvasMessage.pending: .kouAccent
        case CanvasMessage.dropped: .orange
        default: .secondary
        }
    }
}

/// CanvasLog is what happened on the canvas while it was open: what it
/// said, what its agents changed on it.
struct CanvasLog: View {
    let model: CanvasModel

    var body: some View {
        List(model.activity.reversed()) { item in
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: symbol(item))
                    .foregroundStyle(color(item))
                    .frame(width: 20)
                VStack(alignment: .leading, spacing: 3) {
                    Text(item.text)
                        .font(.subheadline)
                        .textSelection(.enabled)
                    Text(item.at, format: .dateTime.hour().minute().second())
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(.vertical, 1)
        }
        .groupedBackground()
        .overlay {
            if model.activity.isEmpty {
                ContentUnavailableView("Nothing yet", systemImage: "list.bullet.rectangle",
                                       description: Text("What the canvas says, and what its agents change on it, shows here while it is open."))
            }
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
        case (.notice, "error"): .red
        case (.notice, "warning"): .orange
        default: .secondary
        }
    }
}
