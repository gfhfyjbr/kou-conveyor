import SwiftUI
import UIKit

// The app's own parts, in the cockpits' manner: mono capitals for what
// labels things, hairlines between them, the accent for what runs. Every
// screen is made of these.

// MARK: - Type

extension Font {
    /// The cockpits' voice: mono capitals, for what labels a thing — a
    /// section, a state, a tag. Set with .tracking(Font.eyebrowTracking)
    /// and .textCase(.uppercase), as Eyebrow does.
    static let kouEyebrow = Font.system(.caption2, design: .monospaced).weight(.semibold)
    /// What tells of a thing in mono: a time, a size, a path, a model.
    static let kouMeta = Font.system(.caption, design: .monospaced)
    /// The tightest mono, for a tag's capitals.
    static let kouTag = Font.system(size: 9.5, weight: .bold, design: .monospaced)

    static let eyebrowTracking: CGFloat = 1.1
}

/// Eyebrow is a label in the cockpits' voice: mono capitals, spaced.
struct Eyebrow: View {
    let text: String
    var color: Color = .kouFg3

    init(_ text: String, color: Color = .kouFg3) {
        self.text = text
        self.color = color
    }

    var body: some View {
        Text(verbatim: text)
            .textCase(.uppercase)
            .font(.kouEyebrow)
            .tracking(Font.eyebrowTracking)
            .foregroundStyle(color)
            .lineLimit(1)
    }
}

/// SectionLabel heads a part of a list: its name in capitals, how many it
/// holds, and a hairline running to the edge.
struct SectionLabel<Trailing: View>: View {
    let title: String
    var count: Int?
    var color: Color = .kouFg3
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: 8) {
            // The words before the line: else the line takes half the row,
            // and a long title is cut short beside it.
            Eyebrow(title, color: color)
                .layoutPriority(1)
            if let count {
                Text(verbatim: "\(count)")
                    .font(.kouEyebrow)
                    .foregroundStyle(Color.kouFg4)
                    .monospacedDigit()
                    .layoutPriority(1)
            }
            Hairline()
            trailing
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

extension SectionLabel where Trailing == EmptyView {
    init(_ title: String, count: Int? = nil, color: Color = .kouFg3) {
        self.init(title: title, count: count, color: color) { EmptyView() }
    }
}

/// Hairline is the thinnest line the screen draws, across.
struct Hairline: View {
    var color: Color = .kouLine
    @Environment(\.displayScale) private var scale

    var body: some View {
        Rectangle()
            .fill(color)
            .frame(height: 1 / max(scale, 1))
            .frame(maxWidth: .infinity)
            .accessibilityHidden(true)
    }
}

// MARK: - Live

/// LiveDot says that something runs: a dot sending out a ring, as a radar
/// does. Still, it is a dot; with less motion the ring stays.
struct LiveDot: View {
    var color: Color = .kouAccent
    var size: CGFloat = 7
    /// Whether it pings: a dot that does not is a state, not a run.
    var live = true
    /// A diamond in place of the dot: what is pinned keeps its mark.
    var diamond = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        mark
            .fill(color)
            .frame(width: size, height: size)
            .background {
                if live && !reduceMotion {
                    mark
                        .stroke(color, lineWidth: 1.2)
                        .frame(width: size, height: size)
                        .keyframeAnimator(initialValue: 0.0, repeating: true) { ring, t in
                            ring
                                .scaleEffect(1 + t * 1.9)
                                .opacity(t >= 1 ? 0 : 0.75 * (1 - t))
                        } keyframes: { _ in
                            CubicKeyframe(1, duration: 1.3)
                            LinearKeyframe(1, duration: 0.5)
                        }
                }
            }
            .frame(width: size * 2.2, height: size * 2.2)
            .accessibilityHidden(true)
    }

    private var mark: AnyShape {
        diamond ? AnyShape(Diamond()) : AnyShape(Circle())
    }
}

/// Diamond is a square on its corner: the cockpits' mark of a pinned thing.
struct Diamond: Shape {
    func path(in rect: CGRect) -> Path {
        var p = Path()
        p.move(to: CGPoint(x: rect.midX, y: rect.minY))
        p.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
        p.addLine(to: CGPoint(x: rect.midX, y: rect.maxY))
        p.addLine(to: CGPoint(x: rect.minX, y: rect.midY))
        p.closeSubpath()
        return p
    }
}

/// RunLight goes round a thing while it runs, as the cockpit's light goes
/// round a running call: a bright head, a fading tail, just inside its
/// edge. With less motion the edge itself says so.
struct RunLight<S: InsettableShape>: ViewModifier {
    let shape: S
    let active: Bool
    var color: Color = .kouAccent
    var lineWidth: CGFloat = 1.5
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content.overlay {
            if active {
                ZStack {
                    shape.strokeBorder(color.opacity(reduceMotion ? 0.7 : 0.32), lineWidth: 1)
                    if !reduceMotion {
                        GeometryReader { proxy in
                            let side = hypot(proxy.size.width, proxy.size.height)
                            AngularGradient(stops: [
                                .init(color: color.opacity(0), location: 0),
                                .init(color: color.opacity(0), location: 0.62),
                                .init(color: color.opacity(0.45), location: 0.9),
                                .init(color: color, location: 0.995),
                                .init(color: color.opacity(0), location: 1),
                            ], center: .center)
                            .frame(width: side, height: side)
                            .keyframeAnimator(initialValue: 0.0, repeating: true) { light, angle in
                                light.rotationEffect(.degrees(angle))
                            } keyframes: { _ in
                                LinearKeyframe(360, duration: 2.6)
                            }
                            .position(x: proxy.size.width / 2, y: proxy.size.height / 2)
                        }
                        .mask(shape.strokeBorder(lineWidth: lineWidth))
                    }
                }
                .allowsHitTesting(false)
                .transition(.opacity)
            }
        }
    }
}

extension View {
    /// runLight sends the run's light round the shape while active.
    func runLight<S: InsettableShape>(_ active: Bool, in shape: S, color: Color = .kouAccent) -> some View {
        modifier(RunLight(shape: shape, active: active, color: color))
    }
}

/// CornerTicks are the cockpits' registration marks (theme.css, .ticks): the
/// four corners of a box drawn over its edge, the accent's while it has the
/// focus. Round corners bend them with it — its own edge, where it turns —
/// and square ones make them the cockpit's L's.
struct CornerTicks: View {
    let radius: CGFloat
    var color: Color = .kouFg4
    /// How far each runs on along the straight edge.
    var arm: CGFloat = 9
    var lineWidth: CGFloat = 1.5

    var body: some View {
        let side = arm + radius
        RoundedRectangle(cornerRadius: radius, style: .continuous)
            .strokeBorder(color, lineWidth: lineWidth)
            .mask {
                Color.clear
                    .overlay(alignment: .topLeading) { Rectangle().frame(width: side, height: side) }
                    .overlay(alignment: .topTrailing) { Rectangle().frame(width: side, height: side) }
                    .overlay(alignment: .bottomLeading) { Rectangle().frame(width: side, height: side) }
                    .overlay(alignment: .bottomTrailing) { Rectangle().frame(width: side, height: side) }
            }
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}

// MARK: - Panels

extension View {
    /// kouPanel sets the view on an outlined panel, as the cockpits draw a
    /// call or a prompt: a ground a step up from the screen's, a hairline
    /// round it.
    func kouPanel(radius: CGFloat = 14, fill: Color = .kouPanel, stroke: Color = .kouLine) -> some View {
        let shape = RoundedRectangle(cornerRadius: .corner(radius), style: .continuous)
        return background(fill, in: shape)
            .overlay(shape.strokeBorder(stroke, lineWidth: 1))
    }

    /// kouRow gives a grouped list's row the shade's ground and hairlines.
    func kouRow() -> some View {
        listRowBackground(Color.kouCard)
            .listRowSeparatorTint(Color.kouLine)
    }

    /// kouHeader stands a plain list's section header on the ground, edge
    /// to edge: pinned at the top as the list scrolls, the rows go under
    /// it, not through it.
    func kouHeader(top: CGFloat = 14) -> some View {
        modifier(KouHeader(top: top))
            .listRowInsets(EdgeInsets())
    }
}

extension EnvironmentValues {
    /// The colour scheme the app's root sees. A pinned header, under a hard
    /// scroll edge, is drawn where iOS says light even in dark: its colours
    /// take this one instead.
    @Entry var kouScheme: ColorScheme?
}

private struct KouHeader: ViewModifier {
    let top: CGFloat
    @Environment(\.kouScheme) private var root
    @Environment(\.colorScheme) private var scheme

    func body(content: Content) -> some View {
        content
            .padding(.horizontal, 20)
            .padding(.top, top)
            .padding(.bottom, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.kouGround)
            .environment(\.colorScheme, root ?? scheme)
    }
}

/// Tag is a word in a box, in mono capitals: a state or a kind, in its tone.
struct Tag: View {
    let text: String
    var tone: Color = .kouFg3
    var filled = false

    init(_ text: String, tone: Color = .kouFg3, filled: Bool = false) {
        self.text = text
        self.tone = tone
        self.filled = filled
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: .corner(5), style: .continuous)
        Text(verbatim: text)
            .textCase(.uppercase)
            .font(.kouTag)
            .tracking(0.8)
            .lineLimit(1)
            .fixedSize()
            .foregroundStyle(filled ? Color.kouAccentInk : tone)
            .padding(.horizontal, 6)
            .padding(.vertical, 3)
            .background(filled ? tone : tone.opacity(0.1), in: shape)
            .overlay(shape.strokeBorder(tone.opacity(filled ? 0 : 0.4), lineWidth: 1))
    }
}

// MARK: - Empty

/// KouEmpty is what a screen shows in place of what it has none of: its
/// mark at the heart of the tunnel's rings, what is missing, and what to do.
struct KouEmpty<Actions: View>: View {
    let symbol: String
    let title: String
    var message: String?
    var tone: Color = .kouFg2
    /// Whether a light goes round the rings: something is on its way.
    var waiting = false
    @ViewBuilder var actions: Actions

    var body: some View {
        VStack(spacing: 0) {
            Rings(symbol: symbol, tone: tone, waiting: waiting)
                .padding(.bottom, 22)
            Text(verbatim: title)
                .font(.title3.weight(.semibold))
                .foregroundStyle(Color.kouFg)
                .multilineTextAlignment(.center)
            if let message {
                Text(verbatim: message)
                    .font(.subheadline)
                    .foregroundStyle(Color.kouFg3)
                    .multilineTextAlignment(.center)
                    .padding(.top, 6)
                    .frame(maxWidth: 320)
            }
            actions
                .padding(.top, 20)
        }
        .padding(.horizontal, 32)
        .padding(.vertical, 24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

extension KouEmpty where Actions == EmptyView {
    init(symbol: String, title: String, message: String? = nil, tone: Color = .kouFg2, waiting: Bool = false) {
        self.init(symbol: symbol, title: title, message: message, tone: tone, waiting: waiting) { EmptyView() }
    }
}

/// Rings are the empty screens' mark: a symbol in a disc, in two rings.
struct Rings: View {
    let symbol: String
    var tone: Color = .kouFg2
    var waiting = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            Circle()
                .strokeBorder(Color.kouLine, style: StrokeStyle(lineWidth: 1, dash: [2, 4]))
                .frame(width: 128, height: 128)
            Circle()
                .strokeBorder(Color.kouLine2, lineWidth: 1)
                .frame(width: 92, height: 92)
            if waiting && !reduceMotion {
                Circle()
                    .fill(Color.kouAccent)
                    .frame(width: 6, height: 6)
                    .offset(y: -46)
                    .keyframeAnimator(initialValue: 0.0, repeating: true) { dot, angle in
                        dot.rotationEffect(.degrees(angle))
                    } keyframes: { _ in
                        LinearKeyframe(360, duration: 3.2)
                    }
            }
            Circle()
                .fill(Color.kouPanel)
                .overlay(Circle().strokeBorder(Color.kouLine, lineWidth: 1))
                .frame(width: 64, height: 64)
            Image(systemName: symbol)
                .font(.system(size: 24, weight: .regular))
                .foregroundStyle(tone)
                .symbolRenderingMode(.hierarchical)
        }
        .frame(width: 128, height: 128)
        .accessibilityHidden(true)
    }
}

// MARK: - Monogram

/// Monogram is a workspace's tile: its initial on a muted colour of its
/// own, from its name — told apart at a glance, quiet beside the accent.
struct Monogram: View {
    let name: String
    var size: CGFloat = 40
    /// Whether it is the one shown: a ring of the accent round it.
    var current = false

    var body: some View {
        let colors = Monogram.colors(for: name)
        let shape = RoundedRectangle(cornerRadius: .corner(size * 0.3), style: .continuous)
        Text(verbatim: initial)
            .font(.system(size: size * 0.44, weight: .semibold, design: .monospaced))
            .foregroundStyle(colors.ink)
            .frame(width: size, height: size)
            .background(colors.ground, in: shape)
            .overlay(shape.strokeBorder(colors.ink.opacity(0.18), lineWidth: 1))
            .padding(current ? 3 : 0)
            .overlay {
                if current {
                    RoundedRectangle(cornerRadius: .corner(size * 0.3) + 3, style: .continuous)
                        .strokeBorder(Color.kouAccent, lineWidth: 1.5)
                }
            }
            .accessibilityHidden(true)
    }

    private var initial: String {
        let letter = name.first(where: { $0.isLetter || $0.isNumber }) ?? "·"
        return String(letter).uppercased()
    }

    private static let cache = MonogramCache()

    /// colors are a name's ground and ink: its hue from the name, the
    /// lightness and chroma the same for all, in light and in dark.
    static func colors(for name: String) -> (ground: Color, ink: Color) {
        if let hit = cache.colors[name] { return hit }
        var hash: UInt64 = 0xcbf29ce484222325
        for byte in name.utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x100000001b3
        }
        let hue = Double(hash % 360)
        let ground = Look.dynamic(light: LookColor.hex(l: 0.9, c: 0.035, h: hue),
                                  dark: LookColor.hex(l: 0.29, c: 0.045, h: hue))
        let ink = Look.dynamic(light: LookColor.hex(l: 0.42, c: 0.09, h: hue),
                               dark: LookColor.hex(l: 0.86, c: 0.075, h: hue))
        let colors = (Color(uiColor: ground), Color(uiColor: ink))
        cache.colors[name] = colors
        return colors
    }
}

/// The monograms' colours, worked out once a name: only the interface
/// reads and writes them, on the main thread.
private final class MonogramCache: @unchecked Sendable {
    var colors: [String: (ground: Color, ink: Color)] = [:]
}

// MARK: - Orbit

/// Orbit is the tunnel at large: the planet in its ring, a signal going
/// round it while the tunnel is up — the pairing's and the relay's mark.
struct Orbit: View {
    var color: Color = .kouAccent
    /// Whether the signal goes round: the tunnel carries.
    var live = true
    var size: CGFloat = 132
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            Circle()
                .fill(RadialGradient(colors: [color.opacity(0.22), color.opacity(0)],
                                     center: .center, startRadius: 0, endRadius: size * 0.55))
            Circle()
                .strokeBorder(Color.kouLine, style: StrokeStyle(lineWidth: 1, dash: [2, 5]))
            Circle()
                .strokeBorder(color.opacity(0.28), lineWidth: 1)
                .padding(size * 0.14)
            if live && !reduceMotion {
                Circle()
                    .fill(color)
                    .frame(width: 7, height: 7)
                    .shadow(color: color, radius: 6)
                    .offset(y: -(size / 2 - size * 0.14))
                    .keyframeAnimator(initialValue: 0.0, repeating: true) { dot, angle in
                        dot.rotationEffect(.degrees(angle))
                    } keyframes: { _ in
                        LinearKeyframe(360, duration: 4.5)
                    }
            }
            PlanetMark(lineWidth: 1.15)
                .foregroundStyle(color)
                .padding(size * 0.27)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

// MARK: - Head title

/// HeadTitle names a screen in its bar only once the screen's own head has
/// scrolled away, as Apple's detail screens do: the name is said once.
private struct HeadTitle: ViewModifier {
    let title: String
    let after: CGFloat
    @State private var shown = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .onScrollGeometryChange(for: Bool.self) { geometry in
                geometry.contentOffset.y + geometry.contentInsets.top > after
            } action: { _, gone in
                withAnimation(reduceMotion ? nil : .easeOut(duration: 0.22)) { shown = gone }
            }
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Text(verbatim: title)
                        .font(.headline)
                        .foregroundStyle(Color.kouFg)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .opacity(shown ? 1 : 0)
                        .offset(y: shown ? 0 : 8)
                        .accessibilityAddTraits(.isHeader)
                }
                .sharedBackgroundVisibility(.hidden)
            }
    }
}

extension View {
    /// headTitle names the screen in its bar once `after` points of it
    /// have scrolled away — its head, which names it first.
    func headTitle(_ title: String, after: CGFloat = 64) -> some View {
        modifier(HeadTitle(title: title, after: after))
    }
}

// MARK: - Settings

/// RowIcon is a settings row's mark: its symbol on a chip of the shade, as
/// quiet as the row — the accent is for what runs, not for every row.
struct RowIcon: View {
    let symbol: String
    var tone: Color = .kouFg2

    init(_ symbol: String, tone: Color = .kouFg2) {
        self.symbol = symbol
        self.tone = tone
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: .corner(7), style: .continuous)
        Image(systemName: symbol)
            .font(.system(size: 13, weight: .semibold))
            .foregroundStyle(tone)
            .frame(width: 28, height: 28)
            .background(Color.kouChip, in: shape)
            .overlay(shape.strokeBorder(Color.kouLine2.opacity(0.7), lineWidth: 1))
            .accessibilityHidden(true)
    }
}

/// FormNote is a grouped list's footer, in the shade's quiet ink.
struct FormNote: View {
    let text: Text

    init(_ key: LocalizedStringKey) { text = Text(key) }
    init(verbatim: String) { text = Text(verbatim: verbatim) }

    var body: some View {
        text
            .font(.footnote)
            .foregroundStyle(Color.kouFg3)
    }
}

/// Reading is one of an instrument strip's: what it reads, in capitals,
/// over its value in mono.
struct Reading<Value: View>: View {
    let label: String
    @ViewBuilder var value: Value

    var body: some View {
        VStack(spacing: 5) {
            Eyebrow(label, color: .kouFg4)
            value
                .font(.system(.subheadline, design: .monospaced, weight: .semibold))
                .lineLimit(1)
                .minimumScaleFactor(0.75)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 12)
        .accessibilityElement(children: .combine)
    }
}

/// ReadingStrip sets readings side by side on a panel, a hairline between
/// them.
struct ReadingStrip<Content: View>: View {
    /// On a panel of its own; not, in a row that is a card already.
    var framed = true
    @ViewBuilder var content: Content
    @Environment(\.displayScale) private var scale

    var body: some View {
        let strip = HStack(spacing: 0) {
            Group(subviews: content) { subviews in
                ForEach(Array(subviews.enumerated()), id: \.offset) { n, subview in
                    if n > 0 {
                        Rectangle()
                            .fill(Color.kouLine)
                            .frame(width: 1 / max(scale, 1))
                            .padding(.vertical, 10)
                    }
                    subview
                }
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        if framed {
            strip.kouPanel(radius: 12, fill: .kouCard)
        } else {
            strip
        }
    }
}

/// KouLabeledContentStyle sets a row's value as the cockpits write what a
/// machine says — in mono, in the second ink — after its label in the
/// first.
struct KouLabeledContentStyle: LabeledContentStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            configuration.label
                .foregroundStyle(Color.kouFg)
            Spacer(minLength: 12)
            configuration.content
                .font(.system(.subheadline, design: .monospaced))
                .foregroundStyle(Color.kouFg2)
                .multilineTextAlignment(.trailing)
        }
        .accessibilityElement(children: .combine)
    }
}

extension LabeledContentStyle where Self == KouLabeledContentStyle {
    static var kou: KouLabeledContentStyle { KouLabeledContentStyle() }
}

/// Meter is a level on a track, as the cockpit's limit bars draw it.
struct Meter: View {
    /// From 0 to 1.
    let value: Double
    var tint: Color = .kouAccent

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: .corner(2), style: .continuous)
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                shape.fill(Color.kouChip)
                shape.fill(tint)
                    .frame(width: value > 0 ? max(3, geometry.size.width * min(value, 1)) : 0)
            }
        }
        .animation(.smooth, value: value)
        .accessibilityHidden(true)
    }
}

/// StateMark is a state as the cockpit's accounts write it: a small square
/// and the word, in mono capitals, in the state's colour — the square
/// blinking while the state is passing.
struct StateMark: View {
    let word: String
    var tone: Color = .kouFg3
    var blinking = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 6) {
            square
            Text(verbatim: word)
                .textCase(.uppercase)
                .font(.kouTag)
                .tracking(1)
                .foregroundStyle(tone)
                .lineLimit(1)
        }
        .fixedSize()
    }

    @ViewBuilder
    private var square: some View {
        let mark = Rectangle()
            .fill(tone)
            .frame(width: 6, height: 6)
        if blinking && !reduceMotion {
            mark.phaseAnimator([1.0, 0.2]) { view, opacity in
                view.opacity(opacity)
            } animation: { _ in
                .easeInOut(duration: 0.7)
            }
        } else {
            mark
        }
    }
}

/// RadioMark is the mark of a choice among others: a ring, lit in the
/// accent with its dot when it is the one chosen.
struct RadioMark: View {
    let chosen: Bool

    var body: some View {
        ZStack {
            Circle()
                .strokeBorder(chosen ? Color.kouAccent : Color.kouLine2, lineWidth: 1.5)
            if chosen {
                Circle()
                    .fill(Color.kouAccent)
                    .padding(5)
                    .transition(.scale.combined(with: .opacity))
            }
        }
        .frame(width: 20, height: 20)
        .animation(.snappy(duration: 0.2), value: chosen)
        .accessibilityHidden(true)
    }
}

// MARK: - Buttons

/// KouButtonStyle is the app's own filled button: the accent, its ink,
/// pressed in a little.
struct KouButtonStyle: ButtonStyle {
    var tone: Color = .kouAccent
    var ink: Color = .kouAccentInk
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.body.weight(.semibold))
            .foregroundStyle(ink)
            .padding(.horizontal, 22)
            .frame(minHeight: 50)
            .frame(maxWidth: .infinity)
            .background(tone.opacity(enabled ? 1 : 0.4), in: .rect(cornerRadius: .corner(16), style: .continuous))
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(.spring(response: 0.25, dampingFraction: 0.7), value: configuration.isPressed)
    }
}

extension ButtonStyle where Self == KouButtonStyle {
    static var kou: KouButtonStyle { KouButtonStyle() }
}

#Preview("Kit") {
    ScrollView {
        VStack(alignment: .leading, spacing: 24) {
            SectionLabel("Pinned", count: 3)
            HStack(spacing: 18) {
                LiveDot()
                LiveDot(diamond: true)
                LiveDot(color: .kouOK, live: false)
                Tag("running", tone: .kouAccent)
                Tag("ready", tone: .kouOK)
                Tag("failed", tone: .kouErr, filled: true)
            }
            Text("Bash  git status")
                .font(.kouMeta)
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .kouPanel()
                .runLight(true, in: RoundedRectangle(cornerRadius: .corner(14), style: .continuous))
            HStack {
                Monogram(name: "kou-conveyor")
                Monogram(name: "website", current: true)
                Monogram(name: "api")
                Monogram(name: "dotfiles")
            }
            Orbit()
            KouEmpty(symbol: "point.3.connected.trianglepath.dotted", title: "No canvases",
                     message: "A canvas wires terminals, agents and sources together.")
        }
        .padding()
    }
    .background(Color.kouGround)
    .preferredColorScheme(.dark)
}

// MARK: - Time

extension Date {
    /// kouAge is how long ago, as a tag says it: "now", "4m", "3h", "2d",
    /// "5w", then the date.
    var kouAge: String {
        let seconds = Date.now.timeIntervalSince(self)
        if self == .distantPast { return "—" }
        if seconds < 60 { return "now" }
        if seconds < 3600 { return "\(Int(seconds / 60))m" }
        if seconds < 86_400 { return "\(Int(seconds / 3600))h" }
        if seconds < 7 * 86_400 { return "\(Int(seconds / 86_400))d" }
        if seconds < 35 * 86_400 { return "\(Int(seconds / (7 * 86_400)))w" }
        let sameYear = Calendar.current.isDate(self, equalTo: .now, toGranularity: .year)
        return formatted(sameYear ? .dateTime.day().month(.abbreviated) : .dateTime.month(.abbreviated).year())
    }
}
