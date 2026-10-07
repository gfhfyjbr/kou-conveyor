import SwiftUI

/// CanvasBoard is a canvas as the cockpit draws it: its nodes where they
/// stand, wired by their edges. It fits the screen, and pinches closer.
struct CanvasBoard: View {
    let model: CanvasModel
    var open: (CanvasNode) -> Void
    @State private var zoom: CGFloat = 1
    @GestureState private var pinch: CGFloat = 1

    var body: some View {
        GeometryReader { geometry in
            let bounds = Self.bounds(of: model.nodes)
            let fit = min(geometry.size.width / bounds.width, geometry.size.height / bounds.height, 1)
            let scale = max(fit * zoom * pinch, 0.05)
            ScrollView([.horizontal, .vertical]) {
                board(bounds)
                    .scaleEffect(scale, anchor: .topLeading)
                    .frame(width: bounds.width * scale, height: bounds.height * scale, alignment: .topLeading)
                    .frame(minWidth: geometry.size.width, minHeight: geometry.size.height)
            }
            .defaultScrollAnchor(.center)
            .scrollIndicators(.hidden)
            .simultaneousGesture(
                MagnifyGesture()
                    .updating($pinch) { value, pinch, _ in pinch = value.magnification }
                    .onEnded { value in zoom = Self.clamp(zoom * value.magnification, fit: fit) }
            )
            .overlay(alignment: .topTrailing) {
                zoomControls(fit: fit)
                    .padding(12)
            }
        }
        .background(Color(uiColor: .systemGroupedBackground))
    }

    private func board(_ bounds: CGRect) -> some View {
        ZStack(alignment: .topLeading) {
            BoardGrid()
            Canvas { context, _ in
                context.translateBy(x: -bounds.minX, y: -bounds.minY)
                for edge in model.edges {
                    guard let from = model.node(edge.from.node)?.frame, let to = model.node(edge.to.node)?.frame else { continue }
                    let live = model.status[edge.from.node]?.state == "busy"
                    Self.draw(edge: from, to: to, live: live, in: &context)
                }
            }
            ForEach(model.nodes) { node in
                NodeCard(node: node, status: model.status[node.id], feed: model.feeds[node.id]?.last, pending: model.pending[node.id] ?? 0)
                    .frame(width: node.frame.width, height: node.frame.height)
                    .offset(x: node.frame.minX - bounds.minX, y: node.frame.minY - bounds.minY)
                    .onTapGesture { open(node) }
            }
        }
        .frame(width: bounds.width, height: bounds.height, alignment: .topLeading)
    }

    private func zoomControls(fit: CGFloat) -> some View {
        GlassEffectContainer(spacing: 8) {
            VStack(spacing: 8) {
                Button("Zoom in", systemImage: "plus") {
                    withAnimation(.smooth) { zoom = Self.clamp(zoom * 1.5, fit: fit) }
                }
                Button("Zoom out", systemImage: "minus") {
                    withAnimation(.smooth) { zoom = Self.clamp(zoom / 1.5, fit: fit) }
                }
                Button("Fit", systemImage: "arrow.down.right.and.arrow.up.left") {
                    withAnimation(.smooth) { zoom = 1 }
                }
                .disabled(zoom == 1)
            }
            .labelStyle(.iconOnly)
            .font(.body.weight(.semibold))
            .buttonStyle(.glass)
            .buttonBorderShape(.circle)
        }
    }

    static func clamp(_ zoom: CGFloat, fit: CGFloat) -> CGFloat {
        // From the whole canvas to three times its own size.
        min(max(zoom, 0.5), max(3 / max(fit, 0.01), 1))
    }

    static func bounds(of nodes: [CanvasNode]) -> CGRect {
        guard let first = nodes.first?.frame else { return CGRect(x: 0, y: 0, width: 600, height: 400) }
        let union = nodes.dropFirst().reduce(first) { $0.union($1.frame) }
        return union.insetBy(dx: -60, dy: -60)
    }

    static func draw(edge from: CGRect, to: CGRect, live: Bool, in context: inout GraphicsContext) {
        let start = CGPoint(x: from.maxX, y: from.midY)
        let end = CGPoint(x: to.minX, y: to.midY)
        let bend = max(60, abs(end.x - start.x) / 2)
        var path = Path()
        path.move(to: start)
        path.addCurve(to: end, control1: CGPoint(x: start.x + bend, y: start.y), control2: CGPoint(x: end.x - bend, y: end.y))
        let color = live ? Color.kouAccent : Color.secondary.opacity(0.6)
        context.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: live ? 4 : 3, lineCap: .round, dash: live ? [14, 10] : []))
        var head = Path()
        head.move(to: CGPoint(x: end.x - 14, y: end.y - 8))
        head.addLine(to: end)
        head.addLine(to: CGPoint(x: end.x - 14, y: end.y + 8))
        context.stroke(head, with: .color(color), style: StrokeStyle(lineWidth: 3, lineCap: .round, lineJoin: .round))
        context.fill(Path(ellipseIn: CGRect(x: start.x - 6, y: start.y - 6, width: 12, height: 12)), with: .color(color))
    }
}

/// BoardGrid is the board's dots.
struct BoardGrid: View {
    var body: some View {
        Canvas { context, size in
            let step: CGFloat = 32
            let dot = Path(ellipseIn: CGRect(x: -1.5, y: -1.5, width: 3, height: 3))
            var y: CGFloat = step / 2
            while y < size.height {
                var x: CGFloat = step / 2
                while x < size.width {
                    context.fill(dot.offsetBy(dx: x, dy: y), with: .color(.secondary.opacity(0.25)))
                    x += step
                }
                y += step
            }
        }
    }
}

/// NodeCard is a node on the board: a pane of glass tinted by what its
/// program does. It is drawn at the canvas's own size; the board scales it.
struct NodeCard: View {
    let node: CanvasNode
    let status: NodeStatus?
    let feed: FeedEntry?
    let pending: Int

    var body: some View {
        let state = status?.state ?? "stopped"
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Image(systemName: NodeKindIcon.symbol(node.kind))
                    .font(.system(size: 26, weight: .semibold))
                    .foregroundStyle(node.kind == CanvasNode.agent ? Color.kouAccent : .primary)
                Text(node.title.isEmpty ? node.kind.capitalized : node.title)
                    .font(.system(size: 26, weight: .semibold))
                    .lineLimit(1)
                Spacer(minLength: 8)
                if pending > 0 {
                    Text("\(pending)")
                        .font(.system(size: 18, weight: .bold, design: .monospaced))
                        .padding(.horizontal, 9)
                        .padding(.vertical, 3)
                        .background(Color.kouAccent, in: .capsule)
                        .foregroundStyle(.white)
                }
                Circle()
                    .fill(StateBadge.color(state))
                    .frame(width: 14, height: 14)
            }
            Text(line(state))
                .font(.system(size: 19, weight: .medium, design: .monospaced))
                .foregroundStyle(StateBadge.color(state))
                .lineLimit(1)
            Text(summary(for: node))
                .font(.system(size: 20))
                .foregroundStyle(.secondary)
                .lineLimit(nil)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .clipped()
        }
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .glassEffect(.regular.tint(StateBadge.color(state).opacity(state == "stopped" || state == "exited" ? 0.04 : 0.16)).interactive(),
                     in: .rect(cornerRadius: 26))
        .contentShape(.rect(cornerRadius: 26))
    }

    private func line(_ state: String) -> String {
        if let activity = status?.activity, !activity.isEmpty { return "\(state) · \(activity)" }
        if let detail = status?.detail, !detail.isEmpty { return "\(state) · \(detail)" }
        return state
    }

    private func summary(for node: CanvasNode) -> String {
        switch node.kind {
        case CanvasNode.note:
            return node.config?["text"]?.string ?? ""
        case CanvasNode.agent:
            if let feed {
                if let tool = feed.tool { return "\(tool.name) \(tool.summary ?? "")" }
                return feed.text ?? ""
            }
            return node.runtime?.agentSession == nil ? "Not started" : "Tap to read and write to it"
        case CanvasNode.terminal:
            return node.config?["command"]?.string ?? node.runtime?.branch.map { "on \($0)" } ?? "Tap to see its screen"
        default:
            return node.config?["command"]?.string ?? ""
        }
    }
}
