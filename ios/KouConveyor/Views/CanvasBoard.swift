import SwiftUI

/// CanvasBoard is a canvas as the cockpit draws it: its nodes where they
/// stand, wired by their edges. The board scales where things stand, not
/// its words: a card shows what its size on the screen has room for — its
/// kind's mark, its title, its state, what it does —, so that a whole
/// canvas stays legible on a phone. The board starts where most cards show
/// their titles, fits the screen on demand, and pinches closer. A tap opens
/// a node — a kou agent's chat, any other's details —; held down, a node
/// offers what can be done to it.
struct CanvasBoard: View {
    let model: CanvasModel
    /// The scale the user chose; nil until they zoom.
    @Binding var chosen: CGFloat?
    var act: (NodeAction) -> Void
    @GestureState private var pinch: CGFloat = 1

    /// The scale a board starts at, unless the whole canvas fits larger.
    static let start: CGFloat = 0.25
    /// The most a board is enlarged: the canvas's own size.
    static let most: CGFloat = 1

    var body: some View {
        GeometryReader { geometry in
            let bounds = Self.bounds(of: model)
            let fit = min(geometry.size.width / bounds.width, geometry.size.height / bounds.height, Self.most)
            let base = chosen ?? max(fit, Self.start)
            let scale = max(Self.clamp(base * pinch, fit: fit), 0.02)
            ScrollView([.horizontal, .vertical]) {
                board(bounds, scale: scale)
                    .frame(minWidth: geometry.size.width, minHeight: geometry.size.height)
            }
            .defaultScrollAnchor(.leading)
            .scrollIndicators(.hidden)
            // The screen's left edge is the way back, not the board's: a
            // swipe from it pulled the board instead of going back.
            .overlay(alignment: .leading) {
                Color.clear
                    .frame(width: 20)
                    .contentShape(.rect)
            }
            .simultaneousGesture(
                MagnifyGesture()
                    .updating($pinch) { value, pinch, _ in pinch = value.magnification }
                    .onEnded { value in chosen = Self.clamp(base * value.magnification, fit: fit) }
            )
            .overlay(alignment: .topTrailing) {
                zoomControls(base: base, fit: fit)
                    .padding(12)
            }
        }
        .background(Color.kouGroupedGround)
    }

    private func board(_ bounds: CGRect, scale: CGFloat) -> some View {
        let place = Placement(origin: bounds.origin, scale: scale)
        return ZStack(alignment: .topLeading) {
            BoardGrid(step: Self.gridStep(at: scale))
            Canvas { context, _ in
                for edge in model.edges {
                    guard let from = model.node(edge.from.node)?.frame, let to = model.node(edge.to.node)?.frame else { continue }
                    Self.draw(Self.wire(from: from, to: to).placed(by: place), style: style(of: edge), in: &context)
                }
            }
            ForEach(model.nodes) { node in
                let frame = place.rect(node.frame)
                Button {
                    act(.open(node))
                } label: {
                    NodeCard(node: node, status: model.status[node.id], feed: model.feeds[node.id]?.last,
                             pending: model.pending(for: node.id), awaiting: model.awaiting(for: node.id), size: frame.size)
                }
                .buttonStyle(.plain)
                .contextMenu {
                    NodeMenu(model: model, node: node, act: act)
                }
                .frame(width: frame.width, height: frame.height)
                .offset(x: frame.minX, y: frame.minY)
            }
        }
        .frame(width: bounds.width * scale, height: bounds.height * scale, alignment: .topLeading)
    }

    private func zoomControls(base: CGFloat, fit: CGFloat) -> some View {
        GlassEffectContainer(spacing: 8) {
            VStack(spacing: 8) {
                zoomButton("Zoom in", "plus") {
                    chosen = Self.clamp(base * 1.5, fit: fit)
                }
                .disabled(base >= Self.most - 0.001)
                zoomButton("Zoom out", "minus") {
                    chosen = Self.clamp(base / 1.5, fit: fit)
                }
                .disabled(base <= Self.least(fit: fit) + 0.001)
                zoomButton("Fit", "arrow.down.right.and.arrow.up.left") {
                    chosen = fit
                }
                .disabled(abs(base - fit) < 0.001)
            }
            .font(.body.weight(.semibold))
            .buttonStyle(.glass)
            .buttonBorderShape(.circle)
        }
    }

    /// A button of the zoom's: its symbol in a square of its own, so that a
    /// flat one — the minus — makes as large a circle as the others.
    private func zoomButton(_ title: String, _ symbol: String, change: @escaping () -> Void) -> some View {
        Button {
            withAnimation(.smooth) { change() }
        } label: {
            Image(systemName: symbol)
                .frame(width: 22, height: 22)
        }
        .accessibilityLabel(title)
    }

    /// How a wire is drawn: by its mode, and lit while its source works.
    enum EdgeStyle {
        case quiet, live, approve, off
    }

    private func style(of edge: CanvasEdge) -> EdgeStyle {
        switch edge.effectiveMode {
        case CanvasEdge.off: .off
        case CanvasEdge.approve: .approve
        default: model.status[edge.from.node]?.state == "busy" ? .live : .quiet
        }
    }

    /// The least scale: the whole canvas, or the starting one when the
    /// canvas fits larger.
    static func least(fit: CGFloat) -> CGFloat {
        min(fit, start)
    }

    static func clamp(_ scale: CGFloat, fit: CGFloat) -> CGFloat {
        min(max(scale, least(fit: fit)), most)
    }

    /// The board's extent: its nodes, and the wires between them, since a
    /// wire that turns back bends out past the nodes it joins.
    static func bounds(of model: CanvasModel) -> CGRect {
        let nodes = model.nodes
        guard let first = nodes.first?.frame else { return CGRect(x: 0, y: 0, width: 600, height: 400) }
        var union = nodes.dropFirst().reduce(first) { $0.union($1.frame) }
        for edge in model.edges {
            guard let from = model.node(edge.from.node)?.frame, let to = model.node(edge.to.node)?.frame else { continue }
            let wire = Self.wire(from: from, to: to)
            for step in 0...20 {
                let point = wire.point(at: CGFloat(step) / 20)
                union = union.union(CGRect(x: point.x, y: point.y, width: 0, height: 0))
            }
        }
        return union.insetBy(dx: -60, dy: -60)
    }

    /// The grid's step on the screen: the canvas's 32 points, doubled while
    /// the dots would crowd.
    static func gridStep(at scale: CGFloat) -> CGFloat {
        var step = max(32 * scale, 1)
        while step < 18 { step *= 2 }
        return step
    }

    /// Placement maps the canvas's points onto the board's.
    struct Placement {
        let origin: CGPoint
        let scale: CGFloat

        func point(_ point: CGPoint) -> CGPoint {
            CGPoint(x: (point.x - origin.x) * scale, y: (point.y - origin.y) * scale)
        }

        func rect(_ rect: CGRect) -> CGRect {
            CGRect(origin: point(rect.origin), size: CGSize(width: rect.width * scale, height: rect.height * scale))
        }
    }

    /// Wire is the curve an edge is drawn along: out of its source's right
    /// side, into its target's left one.
    struct Wire {
        let start, control1, control2, end: CGPoint

        func point(at t: CGFloat) -> CGPoint {
            let u = 1 - t
            let a = u * u * u, b = 3 * u * u * t, c = 3 * u * t * t, d = t * t * t
            return CGPoint(x: a * start.x + b * control1.x + c * control2.x + d * end.x,
                           y: a * start.y + b * control1.y + c * control2.y + d * end.y)
        }

        func placed(by place: Placement) -> Wire {
            Wire(start: place.point(start), control1: place.point(control1), control2: place.point(control2), end: place.point(end))
        }
    }

    static func wire(from: CGRect, to: CGRect) -> Wire {
        let start = CGPoint(x: from.maxX, y: from.midY)
        let end = CGPoint(x: to.minX, y: to.midY)
        let bend = max(60, abs(end.x - start.x) / 2)
        return Wire(start: start, control1: CGPoint(x: start.x + bend, y: start.y),
                    control2: CGPoint(x: end.x - bend, y: end.y), end: end)
    }

    /// Draws a wire placed on the board: its strokes keep their width at
    /// any scale.
    static func draw(_ wire: Wire, style: EdgeStyle, in context: inout GraphicsContext) {
        let start = wire.start, end = wire.end
        var path = Path()
        path.move(to: start)
        path.addCurve(to: end, control1: wire.control1, control2: wire.control2)
        let color: Color = switch style {
        case .live: .kouAccent
        case .approve: .yellow
        case .off: Color.secondary.opacity(0.25)
        case .quiet: Color.secondary.opacity(0.6)
        }
        let dash: [CGFloat] = switch style {
        case .live: [7, 5]
        case .approve: [5, 4]
        case .off: [2, 5]
        case .quiet: []
        }
        context.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: style == .live ? 2.5 : 1.75, lineCap: .round, dash: dash))
        var head = Path()
        head.move(to: CGPoint(x: end.x - 7, y: end.y - 4.5))
        head.addLine(to: end)
        head.addLine(to: CGPoint(x: end.x - 7, y: end.y + 4.5))
        context.stroke(head, with: .color(color), style: StrokeStyle(lineWidth: 1.75, lineCap: .round, lineJoin: .round))
        context.fill(Path(ellipseIn: CGRect(x: start.x - 3.5, y: start.y - 3.5, width: 7, height: 7)), with: .color(color))
        if style == .approve {
            // A hand halfway: its messages wait for the user.
            let middle = wire.point(at: 0.5)
            context.fill(Path(ellipseIn: CGRect(x: middle.x - 10, y: middle.y - 10, width: 20, height: 20)), with: .color(.yellow))
            context.draw(Text(Image(systemName: "hand.raised.fill")).font(.system(size: 10, weight: .bold)).foregroundStyle(Color.black),
                         at: middle)
        }
    }
}

/// BoardGrid is the board's dots.
struct BoardGrid: View {
    var step: CGFloat = 32

    var body: some View {
        Canvas { context, size in
            // All the dots are one path, filled once: a board has thousands,
            // and it is drawn again on every frame of a pinch.
            var dots = Path()
            var y = step / 2
            while y < size.height {
                var x = step / 2
                while x < size.width {
                    dots.addEllipse(in: CGRect(x: x - 1, y: y - 1, width: 2, height: 2))
                    x += step
                }
                y += step
            }
            context.fill(dots, with: .color(.secondary.opacity(0.25)))
        }
    }
}

/// NodeCard is a node on the board: a pane of glass tinted by what its
/// program does. Its words keep their size; the card shows what its size
/// on the screen has room for.
struct NodeCard: View {
    let node: CanvasNode
    let status: NodeStatus?
    let feed: FeedEntry?
    /// The messages that wait for it, and of those, the ones that wait for
    /// the user's approval.
    let pending: Int
    var awaiting = 0
    /// The card's size on the screen.
    var size = CGSize(width: 320, height: 200)

    /// Detail is how much of a card is shown: its kind's mark alone; the
    /// mark over its title; its title with its state under it; and what it
    /// does under that, with the counts of the messages that wait for it.
    enum Detail {
        case mark, title, state, whole
    }

    static func detail(for size: CGSize) -> Detail {
        if size.width < 50 || size.height < 30 { return .mark }
        if size.width < 90 || size.height < 52 { return .title }
        if size.width < 100 || size.height < 80 { return .state }
        return .whole
    }

    var body: some View {
        let state = node.isProposed ? "proposed" : status?.state ?? "stopped"
        let tint = node.isProposed ? Color.yellow : StateBadge.color(state)
        let detail = Self.detail(for: size)
        let corner: CGFloat = .corner(min(16, max(6, min(size.width, size.height) / 5)))
        Group {
            switch detail {
            case .mark:
                mark(tint: tint)
            case .title:
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 4) {
                        Image(systemName: NodeKindIcon.symbol(node.kind))
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(symbolColor)
                        Spacer(minLength: 2)
                        signal(tint: tint, counted: false)
                    }
                    Text(title)
                        .font(.caption2.weight(.semibold))
                        .lineLimit(1)
                        .minimumScaleFactor(0.6)
                }
                .padding(6)
            case .state, .whole:
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        Image(systemName: NodeKindIcon.symbol(node.kind))
                            .font(.footnote.weight(.semibold))
                            .foregroundStyle(symbolColor)
                        Text(title)
                            .font(.footnote.weight(.semibold))
                            .lineLimit(1)
                            .minimumScaleFactor(0.75)
                            .layoutPriority(1)
                        Spacer(minLength: 2)
                        signal(tint: tint, counted: detail == .whole)
                    }
                    HStack(spacing: 4) {
                        Text(line(state))
                            .font(.caption2.monospaced().weight(.medium))
                            .foregroundStyle(tint)
                            .lineLimit(1)
                        if detail == .whole {
                            Spacer(minLength: 2)
                            if awaiting > 0 {
                                badge("\(awaiting)", symbol: "hand.raised.fill", tint: .yellow)
                            }
                            if pending - awaiting > 0 {
                                badge("\(pending - awaiting)", symbol: "tray.full.fill", tint: .kouAccent)
                            }
                        }
                    }
                    if detail == .whole {
                        Text(summary(for: node))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                            .clipped()
                    }
                }
                .padding(detail == .whole ? 12 : 8)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .glassEffect(.regular.tint(tint.opacity(state == "stopped" || state == "exited" ? 0.04 : 0.16)).interactive(),
                     in: .rect(cornerRadius: corner))
        .overlay {
            if node.isProposed {
                RoundedRectangle(cornerRadius: corner)
                    .strokeBorder(Color.yellow.opacity(0.85), style: StrokeStyle(lineWidth: 1.5, dash: [6, 4]))
            }
        }
        .contentShape(.rect(cornerRadius: corner))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title), \(node.kind), \(line(state))")
    }

    private var title: String {
        node.title.isEmpty ? node.kind.capitalized : node.title
    }

    private var symbolColor: Color {
        node.kind == CanvasNode.agent ? .kouAccent : .primary
    }

    /// A card too small for words: its kind's mark, and its signal.
    private func mark(tint: Color) -> some View {
        Image(systemName: NodeKindIcon.symbol(node.kind))
            .font(.system(size: max(9, min(18, min(size.width, size.height) * 0.42)), weight: .semibold))
            .foregroundStyle(symbolColor)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .overlay(alignment: .topTrailing) {
                signal(tint: tint, counted: false)
                    .padding(4)
            }
    }

    /// The node's state as a dot; a hand instead while messages wait for
    /// the user's approval, unless the card counts them.
    @ViewBuilder
    private func signal(tint: Color, counted: Bool) -> some View {
        if awaiting > 0 && !counted {
            Image(systemName: "hand.raised.fill")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Color.yellow)
        } else {
            Circle()
                .fill(tint)
                .frame(width: 7, height: 7)
        }
    }

    private func badge(_ count: String, symbol: String, tint: Color) -> some View {
        Label(count, systemImage: symbol)
            .labelStyle(.titleAndIcon)
            .font(.caption2.monospaced().weight(.bold))
            .padding(.horizontal, 5)
            .padding(.vertical, 1)
            .background(tint, in: .cornerCapsule)
            .foregroundStyle(tint == .yellow ? Color.black : tint == .kouAccent ? Color.kouAccentInk : Color.white)
    }

    private func line(_ state: String) -> String {
        if node.isProposed { return "proposed · waits for you" }
        if let activity = status?.activity, !activity.isEmpty { return "\(state) · \(activity)" }
        if let detail = status?.detail, !detail.isEmpty { return "\(state) · \(detail)" }
        return state
    }

    private func summary(for node: CanvasNode) -> String {
        switch node.kind {
        case CanvasNode.note:
            return node.config?["text"]?.string ?? ""
        case CanvasNode.agent:
            if node.isProposed {
                return node.config?["instructions"]?.string.flatMap { $0.isEmpty ? nil : $0 } ?? "An agent proposed it: hold it down to approve it."
            }
            if let feed {
                if let tool = feed.tool { return "\(tool.name) \(tool.summary ?? "")" }
                return feed.text ?? ""
            }
            if node.runtime?.briefed == true || node.runtime?.adopted == true { return "Tap to open its chat" }
            return node.session == nil ? "Not started" : "Its first prompt starts it"
        case CanvasNode.terminal:
            return node.config?["command"]?.string ?? node.runtime?.branch.map { "on \($0)" } ?? "Tap to see its screen"
        case CanvasNode.source:
            let config = node.config
            if let pattern = config?["pattern"]?.string, !pattern.isEmpty { return pattern }
            if let every = config?["every"]?.string, !every.isEmpty { return "Every \(every)" }
            if let at = config?["at"]?.string, !at.isEmpty { return "At \(at)" }
            if let text = config?["text"]?.string, !text.isEmpty { return text }
            return node.preset == "webhook" ? "Fires on a POST to its address" : "Tap to see it"
        default:
            return node.config?["command"]?.string ?? ""
        }
    }
}
