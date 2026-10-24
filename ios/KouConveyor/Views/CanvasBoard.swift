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
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

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
            // A live wire's dashes march to its target, as the cockpit's
            // do; the timeline stands still while none is live, or with
            // less motion.
            let marching = !reduceMotion && model.edges.contains { style(of: $0) == .live }
            TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !marching)) { timeline in
                let phase = marching ? timeline.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 1) * 24 : 0
                Canvas { context, _ in
                    for edge in model.edges {
                        guard let from = model.node(edge.from.node)?.frame, let to = model.node(edge.to.node)?.frame else { continue }
                        Self.draw(Self.wire(from: from, to: to).placed(by: place), style: style(of: edge), phase: phase, in: &context)
                    }
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
            // The board's own controls, in its ink: the accent is for what
            // runs on it.
            .tint(Color.kouFg)
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
    static func draw(_ wire: Wire, style: EdgeStyle, phase: CGFloat = 0, in context: inout GraphicsContext) {
        let start = wire.start, end = wire.end
        var path = Path()
        path.move(to: start)
        path.addCurve(to: end, control1: wire.control1, control2: wire.control2)
        // As the cockpit draws its wires: in the faint ink at rest, dashed
        // in the warning's while its messages wait for the user, sparse
        // while off, and lit in the accent while its source works.
        let color: Color = switch style {
        case .live: .kouAccent
        case .approve: .kouWarn
        case .off: Color.kouFg4.opacity(0.55)
        case .quiet: .kouFg4
        }
        let dash: [CGFloat] = switch style {
        case .live, .approve: [7, 5]
        case .off: [2, 6]
        case .quiet: []
        }
        context.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: style == .live ? 2.25 : 1.5, lineCap: .round, dash: dash,
                                                                     dashPhase: style == .live ? -phase : 0))
        var head = Path()
        head.move(to: CGPoint(x: end.x - 7, y: end.y - 4.5))
        head.addLine(to: end)
        head.addLine(to: CGPoint(x: end.x - 7, y: end.y + 4.5))
        context.stroke(head, with: .color(color), style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
        // The port it leaves by, ringed in the board's ground.
        let port = CGRect(x: start.x - 3.5, y: start.y - 3.5, width: 7, height: 7)
        context.fill(Path(ellipseIn: port), with: .color(color))
        context.stroke(Path(ellipseIn: port.insetBy(dx: -1, dy: -1)), with: .color(.kouGroupedGround), lineWidth: 1.5)
        if style == .approve {
            // A hand halfway: its messages wait for the user.
            let middle = wire.point(at: 0.5)
            let disc = CGRect(x: middle.x - 10, y: middle.y - 10, width: 20, height: 20)
            context.fill(Path(ellipseIn: disc), with: .color(.kouWarn))
            context.stroke(Path(ellipseIn: disc.insetBy(dx: -1, dy: -1)), with: .color(.kouGroupedGround), lineWidth: 2)
            context.draw(Text(Image(systemName: "hand.raised.fill")).font(.system(size: 10, weight: .bold)).foregroundStyle(Color.kouGroupedGround),
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
                    dots.addEllipse(in: CGRect(x: x - 0.9, y: y - 0.9, width: 1.8, height: 1.8))
                    x += step
                }
                y += step
            }
            // The cockpit's dots: its ink, faintly.
            context.fill(dots, with: .color(Color.kouFg.opacity(0.13)))
        }
    }
}

/// NodeCard is a node on the board, as the cockpit draws one: a panel under
/// a header strip — its kind's glyph, its title, its state's square — with
/// what it does below; a line runs along the strip while it works. Its words
/// keep their size; the card shows what its size on the screen has room for.
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
    /// mark over its title; its header with its state under it; and what it
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

    private var state: String { node.isProposed ? "proposed" : status?.state ?? "stopped" }
    private var isNote: Bool { node.kind == CanvasNode.note }
    private var working: Bool { state == "busy" || state == "starting" }

    var body: some View {
        let detail = Self.detail(for: size)
        let shape = RoundedRectangle(cornerRadius: .corner(min(12, max(4, min(size.width, size.height) / 6))), style: .continuous)
        Group {
            switch detail {
            case .mark: mark
            case .title: small
            case .state: card(whole: false)
            case .whole: card(whole: true)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background {
            ZStack {
                shape.fill(Color.kouPanel)
                if isNote { shape.fill(Color.kouWarn.opacity(0.05)) }
            }
        }
        .clipShape(shape)
        .overlay(shape.strokeBorder(edge, style: StrokeStyle(lineWidth: 1, dash: node.isProposed ? [5, 3] : [])))
        .shadow(color: .black.opacity(0.22), radius: 8, y: 5)
        .contentShape(shape)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title), \(node.kind), \(line)")
    }

    /// Its outline: the stronger line; the warning's while it waits or is
    /// proposed, the error's when it failed, the accent's while it works.
    private var edge: Color {
        switch state {
        case "proposed", "waiting": Color.kouWarn.opacity(0.6)
        case "error": Color.kouErr.opacity(0.6)
        case "busy", "starting": Color.kouAccent.opacity(0.45)
        default: isNote ? Color.kouWarn.opacity(0.22) : Color.kouLine2
        }
    }

    /// A card with room for words: its header, then its state and what it
    /// does, or — with room for it — what it says, over a foot of what it
    /// does now and what waits for it.
    private func card(whole: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            head(whole: whole)
            Group {
                if whole {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(summary(for: node))
                            .font(.caption)
                            .foregroundStyle(node.isProposed ? Color.kouFg3 : Color.kouFg2)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                            .clipped()
                        foot
                    }
                } else {
                    stateLine
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, whole ? 8 : 7)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background {
                if node.isProposed {
                    Hatch().stroke(Color.kouWarn.opacity(0.14), lineWidth: 1)
                }
            }
        }
    }

    /// The header strip: what it is and its state, on the raised ground
    /// over a hairline — the warning's for a note, which has no state.
    private func head(whole: Bool) -> some View {
        HStack(spacing: 7) {
            glyph
                .font(.system(size: 11, weight: .semibold))
            Text(title)
                .font(.system(size: 12.5, weight: .semibold))
                .foregroundStyle(Color.kouFg)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .layoutPriority(1)
            Spacer(minLength: 4)
            // Without room for its word, the state is said under the
            // header, the square with it: the header keeps the hand alone.
            if whole {
                signal(word: true)
            } else if awaiting > 0 {
                signal(word: false)
            }
        }
        .padding(.horizontal, 9)
        .frame(height: 28)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(isNote ? Color.clear : Color.kouSurface)
        .overlay(alignment: .bottom) {
            ZStack(alignment: .leading) {
                Rectangle()
                    .fill(isNote ? Color.kouWarn.opacity(0.2) : Color.kouLine)
                if state == "busy" {
                    ScanLine()
                }
            }
            .frame(height: 1)
        }
    }

    /// Under a header with no room for the word: the state — its square
    /// and its word, whole — and what it does, where it has room for a few
    /// letters at least; a note's first words.
    @ViewBuilder
    private var stateLine: some View {
        if isNote {
            Text(summary(for: node))
                .font(.caption)
                .foregroundStyle(Color.kouFg2)
                .lineLimit(1)
        } else {
            let badge = StateBadge(state: state)
            if let doing {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 7) {
                        badge
                        Text(verbatim: doing)
                            .font(.system(size: 10, design: .monospaced))
                            .foregroundStyle(working ? Color.kouAccent : Color.kouFg3)
                            .lineLimit(1)
                            // Measured as its first letters, it shows as many
                            // as fit.
                            .frame(minWidth: 0, idealWidth: 40, maxWidth: .infinity, alignment: .leading)
                    }
                    badge
                }
            } else {
                badge
            }
        }
    }

    /// What it does now, and the counts of what waits for it.
    @ViewBuilder
    private var foot: some View {
        let queued = pending - awaiting
        if doing != nil || awaiting > 0 || queued > 0 {
            HStack(spacing: 5) {
                if let doing {
                    Text(verbatim: doing)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(working ? Color.kouAccent : Color.kouFg3)
                        .lineLimit(1)
                }
                Spacer(minLength: 2)
                if awaiting > 0 {
                    CountBadge(count: awaiting, symbol: "hand.raised.fill", tint: .kouWarn, size: 10)
                }
                if queued > 0 {
                    CountBadge(count: queued, symbol: "tray.full.fill", tint: .kouFg2, size: 10)
                }
            }
        }
    }

    /// What it does now: its activity, or what its state says of itself.
    private var doing: String? {
        if node.isProposed { return "waits for you" }
        if let activity = status?.activity, !activity.isEmpty { return activity }
        if let detail = status?.detail, !detail.isEmpty { return detail }
        return nil
    }

    private var line: String {
        doing.map { "\(state) · \($0)" } ?? state
    }

    private var title: String {
        node.title.isEmpty ? node.kind.capitalized : node.title
    }

    /// Its kind's glyph: the accent's for an agent and for what works, the
    /// warning's for a note, the quiet ink for the rest — as the cockpit's.
    private var glyph: some View {
        Image(systemName: NodeKindIcon.symbol(node.kind))
            .foregroundStyle(isNote ? Color.kouWarn : node.kind == CanvasNode.agent || working ? Color.kouAccent : Color.kouFg3)
    }

    /// A card too small for words: its kind's mark, and its signal.
    private var mark: some View {
        glyph
            .font(.system(size: max(9, min(18, min(size.width, size.height) * 0.42)), weight: .semibold))
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .overlay(alignment: .topTrailing) {
                signal(word: false)
                    .padding(4)
            }
    }

    /// A card with room for its title alone: its mark and signal over it.
    private var small: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 4) {
                glyph
                    .font(.system(size: 9.5, weight: .semibold))
                Spacer(minLength: 2)
                signal(word: false)
            }
            Text(title)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Color.kouFg)
                .lineLimit(1)
                .minimumScaleFactor(0.6)
        }
        .padding(6)
    }

    /// The node's state: its square, with the word where there is room; a
    /// hand instead while messages wait for the user's approval, unless the
    /// card counts them. A note has none.
    @ViewBuilder
    private func signal(word: Bool) -> some View {
        if isNote {
            EmptyView()
        } else if awaiting > 0 && !word {
            Image(systemName: "hand.raised.fill")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Color.kouWarn)
        } else {
            StateBadge(state: state, word: word)
        }
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

/// ScanLine runs along a working node's hairline, as the cockpit's does: a
/// stretch of the accent crossing it. With less motion it lies still.
struct ScanLine: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        GeometryReader { geometry in
            let width = geometry.size.width
            let bar = max(width * 0.28, 12)
            if reduceMotion {
                Rectangle()
                    .fill(Color.kouAccent.opacity(0.7))
            } else {
                Rectangle()
                    .fill(LinearGradient(colors: [Color.kouAccent.opacity(0), .kouAccent, .kouAccent], startPoint: .leading, endPoint: .trailing))
                    .frame(width: bar)
                    .keyframeAnimator(initialValue: 0.0, repeating: true) { line, t in
                        line.offset(x: -bar + t * (width + bar))
                    } keyframes: { _ in
                        LinearKeyframe(1, duration: 1.6)
                    }
            }
        }
        .clipped()
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// Hatch is the stripes over what an agent proposed: not the canvas's yet.
struct Hatch: Shape {
    var gap: CGFloat = 10

    func path(in rect: CGRect) -> Path {
        var path = Path()
        var x = rect.minX - rect.height
        while x < rect.maxX {
            path.move(to: CGPoint(x: x, y: rect.maxY))
            path.addLine(to: CGPoint(x: x + rect.height, y: rect.minY))
            x += gap
        }
        return path
    }
}
