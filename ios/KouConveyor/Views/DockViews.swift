import SwiftUI
import UIKit

/// Dock is what stands on the composer, as the cockpit's dock has it: what
/// the run does — with the summary a compaction writes, as it comes — and
/// what waits in the queue. Each is a tray a little narrower than the
/// composer's panel, its round corners clear of the panel's; each comes out
/// from under what is below it, the composer or the tray between, those
/// above going up for it, and goes back under it.
struct Dock: View {
    let model: SessionModel
    /// Edits a queued message.
    var edit: (QueueItem) -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    // What the dock shows of the model, changed with the trays' motion: the
    // bar the dock stands in grows and shrinks with them, the composer stays
    // where it is, and the transcript goes up and down with the bar. The
    // model's own changes come with none, and a motion given in here would
    // move the trays alone, the bar jumping.
    @State private var running = false
    @State private var compaction: SessionModel.Compaction?
    @State private var items: [QueueItem] = []

    var body: some View {
        // A tray comes up from below its place as the dock's top edge goes
        // up for it, and goes back down as the edge comes down: what is
        // below the dock's foot — the composer's edge — is cut there, so it
        // comes out from under the composer, and goes back under it. Each
        // tray's bottom edge is under the top edge of the one below, drawn
        // later: one that comes between comes out from under the lower.
        VStack(spacing: -1) {
            if running {
                RunTray(activity: model.activity, started: model.started, reconnecting: model.reconnecting,
                        compaction: model.compaction, shown: compaction, top: true)
                    .transition(Self.rising(reduceMotion))
            }
            if !items.isEmpty {
                QueueTray(model: model, items: items, paused: model.queue?.paused == true, running: running,
                          top: !running, edit: edit)
                    .transition(Self.rising(reduceMotion))
            }
        }
        // As wide empty as with trays: its edges do not close in from the
        // sides as the last tray goes.
        .frame(maxWidth: .infinity)
        .padding(.horizontal, Self.inset)
        .clipped()
        // The lowest tray's bottom edge goes under the composer's.
        .padding(.bottom, running || !items.isEmpty ? -1 : 0)
        // As the screen comes, what is there is there already (the first
        // call, old and new the same): no tray rises as it is pushed in.
        .onChange(of: model.isBusy, initial: true) { old, busy in
            guard busy != running else { return }
            move(comes: busy, at: old != busy) { running = busy }
        }
        .onChange(of: model.compaction, initial: true) { old, new in
            // The summary as it streams changes the text, not the tray.
            if (new == nil) == (compaction == nil) {
                compaction = new
            } else {
                move(comes: new != nil, at: old != new) { compaction = new }
            }
        }
        .onChange(of: model.queued, initial: true) { old, new in
            guard new != items else { return }
            move(comes: new.count >= items.count, at: old != new) { items = new }
        }
    }

    /// move changes what the dock shows with the motion of a tray that
    /// comes, or goes; at once, if not moved (as the screen comes).
    private func move(comes: Bool, at moved: Bool, _ change: () -> Void) {
        guard moved else { return change() }
        withAnimation(reduceMotion ? .smooth(duration: 0.25) : Self.motion(comes), change)
    }

    /// How far in from the screen's edges the trays are: past the
    /// composer's margin and its round corners, a little narrower than it
    /// when square.
    static var inset: CGFloat {
        let radius = CGFloat.corner(ComposerMetrics.radius)
        return 10 + max(radius + 6, 14)
    }

    /// A tray comes out as the cockpit's does: quick, then settling in place.
    static let rise = Animation.timingCurve(0.22, 1, 0.36, 1, duration: 0.5)
    /// And goes back under quicker.
    static let sink = Animation.smooth(duration: 0.26)

    /// The motion of a tray that comes, or goes.
    static func motion(_ comes: Bool) -> Animation { comes ? rise : sink }

    /// How a tray, or a part of one, comes and goes: up from under what is
    /// below it — from as far down as it is high, its top edge with the
    /// edge going up over it — and back down under it. With less motion, it
    /// fades in and out.
    static func rising(_ reduceMotion: Bool) -> AnyTransition {
        reduceMotion ? .opacity : .move(edge: .bottom)
    }
}

/// TrayBack is a tray's ground and edge: round on top when nothing is over
/// it. Its bottom edge is under what is below it.
private struct TrayBack: ViewModifier {
    let top: Bool
    var edge: Color = .kouLine2
    /// The run's light round its edge, while it runs; nil: none.
    var light: Color?

    func body(content: Content) -> some View {
        let radius: CGFloat = top ? .corner(14) : 0
        let shape = UnevenRoundedRectangle(topLeadingRadius: radius, topTrailingRadius: radius, style: .continuous)
        content
            .background(Color.kouPanel, in: shape)
            .clipShape(shape)
            .overlay(shape.strokeBorder(edge, lineWidth: 1))
            .runLight(light != nil, in: shape, color: light ?? .kouAccent)
    }
}

// MARK: - The run

/// RunTray says what the agent does while a run goes on. While the run
/// compacts the context it shows the summary written meanwhile, as it comes:
/// the end of it, three lines, those before fading out at the top — three
/// bars until it starts.
struct RunTray: View {
    let activity: String?
    let started: Date?
    let reconnecting: Bool
    /// The compaction going on, as it is now: what the head says, with what
    /// the run does — both at once.
    let compaction: SessionModel.Compaction?
    /// The compaction whose summary the tray shows: the dock's, which comes
    /// and goes with the tray's motion — and keeps the last of it as it goes.
    let shown: SessionModel.Compaction?
    /// Nothing is over it: its top corners are round.
    let top: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let summary = compaction.map(Summary.init)
        // The compaction's own words while it is what the run does: a
        // request to the model tried again meanwhile says so instead.
        let compacting = summary != nil && !reconnecting && activity == SessionModel.compactingActivity
        VStack(alignment: .leading, spacing: 0) {
            head(summary: summary, compacting: compacting)
            if shown != nil, let body = summary ?? shown.map(Summary.init) {
                // Below the head, it comes out from under the tray's bottom
                // edge as the tray grows up for it.
                SummaryBody(summary: body)
                    .padding(.bottom, 10)
                    .transition(Dock.rising(reduceMotion))
            }
        }
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .modifier(TrayBack(top: top, light: reconnecting ? .kouWarn : .kouAccent))
        .accessibilityElement(children: .combine)
    }

    private func head(summary: Summary?, compacting: Bool) -> some View {
        HStack(spacing: 8) {
            // One mark in the other's place, crossfading where it stands.
            ZStack {
                if compacting {
                    PulseMark()
                        .transition(.opacity)
                } else {
                    LiveDot(color: reconnecting ? .kouWarn : .kouAccent, size: 6)
                        .transition(.opacity)
                }
            }
            .frame(width: 6 * 2.2, height: 6 * 2.2)
            Text(reconnecting ? "Reconnecting…" : (compacting ? "Compacting…" : (activity ?? "Working")))
                .font(.system(.footnote, design: .monospaced))
                .foregroundStyle(Color.kouFg2)
                .lineLimit(1)
                .contentTransition(.opacity)
            Spacer(minLength: 8)
            if let part = summary?.part {
                Text(part)
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg4)
                    .transition(.opacity)
            }
            if let started {
                Text(started, style: .timer)
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg4)
                    .monospacedDigit()
            }
        }
        .frame(height: 34)
        .animation(.smooth(duration: 0.25), value: activity)
        .animation(.smooth(duration: 0.25), value: compacting)
        .animation(.smooth(duration: 0.25), value: summary?.part)
    }
}

/// PulseMark is the compaction's mark, as the cockpit's: a square of the
/// accent, pulsing — still, with less motion. As wide as the run's dot.
private struct PulseMark: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let square = RoundedRectangle(cornerRadius: min(CGFloat.corner(3), 2), style: .continuous)
            .fill(Color.kouAccent)
            .frame(width: 6, height: 6)
        Group {
            if reduceMotion {
                square
            } else {
                square.keyframeAnimator(initialValue: 1.0, repeating: true) { view, opacity in
                    view.opacity(opacity)
                } keyframes: { _ in
                    CubicKeyframe(0.25, duration: 0.6)
                    CubicKeyframe(1, duration: 0.6)
                }
            }
        }
        .frame(width: 6 * 2.2, height: 6 * 2.2)
        .accessibilityHidden(true)
    }
}

/// Summary is what a compaction's tray shows of what streamed of it, as the
/// cockpit's: the end of its latest piece — the thinking before the summary,
/// or its text — without the tags round the analysis and the summary, nor
/// blank lines, which its few lines have no room for; and the part it is in.
struct Summary: Equatable {
    let text: String
    let part: String?
    let thinking: Bool

    /// How much of the end of a piece the tray holds.
    static let keep = 2000

    init(_ compaction: SessionModel.Compaction) {
        let piece = compaction.pieces.last
        let raw = piece?.text ?? ""
        thinking = piece?.thinking == true
        part = thinking ? "thinking" : raw.contains("<summary>") ? "summary" : raw.contains("<analysis>") ? "analysis" : nil
        text = Self.shown(raw)
    }

    /// shown is the end of a piece as the tray shows it.
    static func shown(_ raw: String) -> String {
        // Of its end only: what goes of it is less than it keeps over.
        var text = String(raw.suffix(keep * 2))
        for tag in ["<analysis>", "</analysis>", "<summary>", "</summary>"] where text.contains(tag) {
            text = text.replacingOccurrences(of: tag, with: "")
        }
        // A tag still coming.
        if let open = text.lastIndex(of: "<") {
            let rest = text[text.index(after: open)...]
            let name = rest.hasPrefix("/") ? rest.dropFirst() : rest
            if name.allSatisfy({ $0.isASCII && $0.isLowercase }) { text.removeSubrange(open...) }
        }
        // No blank lines; the line being written stays as it is.
        var lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        let last = lines.popLast()
        lines.removeAll { $0.allSatisfy(\.isWhitespace) }
        if let last { lines.append(last) }
        let joined = lines.joined(separator: "\n")
        return String(joined.drop(while: \.isWhitespace).suffix(keep))
    }
}

/// SummaryBody is the end of what a compaction wrote: three lines, the
/// lines above them fading out at the top, the accent's caret after the
/// last word — three bars until it starts.
private struct SummaryBody: View {
    let summary: Summary
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// The text is taller than the body: the lines out of sight fade.
    @State private var over = false
    /// A line of the body.
    @ScaledMetric(relativeTo: .caption2) private var line: CGFloat = 16

    /// How long the caret is lit, and dark.
    static let blink = 0.55

    var body: some View {
        let height = line * 3
        Group {
            if summary.text.isEmpty {
                bars
            } else {
                TimelineView(.periodic(from: .now, by: Self.blink)) { context in
                    let lit = reduceMotion || Int(context.date.timeIntervalSinceReferenceDate / Self.blink) % 2 == 0
                    lines(caret: lit)
                }
                .onGeometryChange(for: Bool.self) { $0.size.height > height + 1 } action: { over = $0 }
            }
        }
        .frame(maxWidth: .infinity, minHeight: height, maxHeight: height, alignment: .bottomLeading)
        .clipped()
        .mask {
            LinearGradient(colors: [over ? .clear : .black, .black], startPoint: .top, endPoint: UnitPoint(x: 0.5, y: 0.34))
        }
        .accessibilityLabel(summary.text.isEmpty ? "The summary is starting" : "The summary, as it is written")
    }

    private func lines(caret: Bool) -> some View {
        let words = Text(verbatim: summary.text)
        let mark = Text(verbatim: "▍").foregroundStyle(caret ? Color.kouAccent : Color.clear)
        return Text("\(words)\(mark)")
            .font(.system(.caption2, design: .monospaced))
            .lineSpacing(3)
            .foregroundStyle(summary.thinking ? Color.kouFg4 : Color.kouFg2)
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
    }

    /// Three bars, the light running over them, until the summary starts.
    private var bars: some View {
        VStack(alignment: .leading, spacing: 0) {
            bar(1)
            bar(0.82)
            bar(0.56)
        }
        .modifier(Shimmer(active: !reduceMotion))
    }

    private func bar(_ width: CGFloat) -> some View {
        RoundedRectangle(cornerRadius: .corner(1.5), style: .continuous)
            .fill(Color.kouLine2)
            .frame(height: 3)
            .scaleEffect(x: width, anchor: .leading)
            .frame(height: line)
    }
}

// MARK: - The queue

/// QueueTray is what waits for the agent: messages sent while it worked —
/// those forced in first, on their way to it, then the queued, in the order
/// they run — each to put to it now, edit, move or drop.
struct QueueTray: View {
    /// What the tray's actions go to.
    let model: SessionModel
    // What it shows, given: going back under, it keeps them — the model's
    // queue is empty by then.
    let items: [QueueItem]
    let paused: Bool
    /// Whether a run goes on, which the queue runs after.
    let running: Bool
    /// Nothing is over it: its top corners are round.
    let top: Bool
    var edit: (QueueItem) -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// So many rows show at once; more scroll.
    static let shown = 3
    /// A row's height.
    static let row: CGFloat = 40

    var body: some View {
        // The queue waits for the user: paused, or with no run to follow.
        let waiting = paused || !running
        VStack(spacing: 0) {
            // Over the rows: one that comes before them comes out from
            // under it as it goes up, one that goes from there goes under it.
            VStack(spacing: 0) {
                head(items, paused: paused, waiting: waiting)
                Hairline()
            }
            .background(Color.kouPanel)
            .zIndex(1)
            list(items, waiting: waiting)
        }
        .overlay(alignment: .leading) {
            if paused {
                Color.kouWarn
                    .frame(width: 2)
                    .transition(.opacity)
            }
        }
        .modifier(TrayBack(top: top, edge: paused ? Color.kouWarn.opacity(0.45) : .kouLine2))
        .animation(.smooth(duration: 0.3), value: paused)
    }

    private func head(_ items: [QueueItem], paused: Bool, waiting: Bool) -> some View {
        HStack(spacing: 8) {
            Eyebrow("Queue", color: paused ? .kouWarn : .kouFg3)
            Text(verbatim: "\(items.count)")
                .font(.system(size: 10, weight: .bold, design: .monospaced))
                .monospacedDigit()
                .contentTransition(.numericText())
                .foregroundStyle(Color.kouAccentInk)
                .padding(.horizontal, 4)
                .frame(minWidth: 18, minHeight: 16)
                .background(paused ? Color.kouWarn : Color.kouAccent,
                            in: RoundedRectangle(cornerRadius: min(CGFloat.corner(4), 8), style: .continuous))
            Text(state(items, paused: paused, waiting: waiting))
                .font(.kouMeta)
                .foregroundStyle(paused ? Color.kouWarn : Color.kouFg3)
                .lineLimit(1)
                .contentTransition(.opacity)
                .frame(maxWidth: .infinity, alignment: .leading)
            // As the cockpit's: what waits for the user, the user runs.
            if waiting, items.contains(where: { $0.forced != true }) {
                Button {
                    Task { await model.resumeQueue() }
                } label: {
                    Text("Resume")
                        .textCase(.uppercase)
                        .font(.kouEyebrow)
                        .tracking(Font.eyebrowTracking)
                        .foregroundStyle(paused ? Color.kouWarn : Color.kouFg)
                        .padding(.horizontal, 9)
                        .frame(height: 24)
                        .overlay(RoundedRectangle(cornerRadius: .corner(7), style: .continuous)
                            .strokeBorder(paused ? Color.kouWarn.opacity(0.5) : Color.kouLine2, lineWidth: 1))
                        .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .transition(.opacity.combined(with: .scale(scale: 0.8)))
                .accessibilityLabel("Resume the queue")
            }
            menu(waiting: waiting)
        }
        .padding(.leading, 12)
        .padding(.trailing, 2)
        .frame(height: 34)
        .animation(.smooth(duration: 0.25), value: items.count)
        .animation(.smooth(duration: 0.25), value: waiting)
    }

    /// What the queue does, as the cockpit's says it — in fewer words, for a
    /// phone's line: what waits for the user has its Resume beside it.
    private func state(_ items: [QueueItem], paused: Bool, waiting: Bool) -> String {
        let forced = items.filter { $0.forced == true }.count
        let queued = items.count - forced
        if waiting { return paused ? "Paused" : "Waiting" }
        if forced > 0, queued > 0 { return "⚡ \(forced) in · \(queued) after" }
        if forced > 0 { return "⚡ Forced in" }
        return "When the agent finishes"
    }

    private func menu(waiting: Bool) -> some View {
        Menu {
            if waiting {
                Button("Resume the queue", systemImage: "play.fill") { Task { await model.resumeQueue() } }
            } else {
                Button("Pause the queue", systemImage: "pause.fill") { Task { await model.pauseQueue() } }
            }
            Button("Clear the queue", systemImage: "trash", role: .destructive) { Task { await model.clearQueue() } }
        } label: {
            Image(systemName: "ellipsis")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(Color.kouFg2)
                .frame(width: 32, height: 32)
                .contentShape(.rect)
        }
        .accessibilityLabel("Queue")
    }

    @ViewBuilder
    private func list(_ items: [QueueItem], waiting: Bool) -> some View {
        // Each in its place in line: the forced ones go in before it.
        let places: [Int?] = {
            var place = 0
            return items.map { item -> Int? in
                guard item.forced != true else { return nil }
                place += 1
                return place
            }
        }()
        let rows = VStack(spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                // The last comes up from under the composer, and goes back
                // under it; one before it, from under the head, as the head
                // goes up for it.
                row(item, place: places[index], index: index, count: items.count, next: !waiting && places[index] == 1)
                    .transition(index == items.count - 1 ? Dock.rising(reduceMotion) : .opacity)
            }
        }
        if items.count > Self.shown {
            ScrollView {
                rows
            }
            .frame(height: Self.row * (CGFloat(Self.shown) + 0.5))
            .scrollBounceBehavior(.basedOnSize)
        } else {
            rows
        }
    }

    private func row(_ item: QueueItem, place: Int?, index: Int, count: Int, next: Bool) -> some View {
        let forced = item.forced == true
        return HStack(spacing: 8) {
            Group {
                if let place {
                    Text(verbatim: String(format: "%02d", place))
                        .font(.system(size: 10.5, weight: .bold, design: .monospaced))
                        .foregroundStyle(Color.kouFg4)
                } else {
                    Image(systemName: "bolt.fill")
                        .font(.system(size: 10, weight: .bold))
                        .foregroundStyle(Color.kouAccent)
                }
            }
            .frame(width: 24)
            Text(item.text)
                .font(.subheadline)
                .foregroundStyle(forced ? Color.kouFg : Color.kouFg2)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            if let images = item.images, !images.isEmpty {
                Label("\(images.count)", systemImage: "photo")
                    .labelStyle(.titleAndIcon)
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg4)
                    .accessibilityLabel("\(images.count) image\(images.count == 1 ? "" : "s")")
            }
            if next {
                Text("Next")
                    .textCase(.uppercase)
                    .font(.kouEyebrow)
                    .tracking(Font.eyebrowTracking)
                    .foregroundStyle(Color.kouFg3)
            }
            if forced {
                Color.clear.frame(width: 4, height: 1)
            } else {
                Menu {
                    actions(item, index: index, count: count)
                } label: {
                    Image(systemName: "ellipsis")
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(Color.kouFg3)
                        .frame(width: 32, height: 32)
                        .contentShape(.rect)
                }
                .accessibilityLabel("Actions")
            }
        }
        .padding(.leading, 6)
        .padding(.trailing, 2)
        .frame(minHeight: Self.row)
        // One on its way in: the accent's edge, as the cockpit's.
        .background {
            if forced { Color.kouAccent.opacity(0.07) }
        }
        .overlay(alignment: .leading) {
            if forced { Color.kouAccent.frame(width: 2) }
        }
        .overlay(alignment: .top) {
            if index > 0 { Hairline() }
        }
        .contentShape(.rect)
        .contextMenu {
            if !forced { actions(item, index: index, count: count) }
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private func actions(_ item: QueueItem, index: Int, count: Int) -> some View {
        Button("Put to the agent now", systemImage: "bolt.fill") {
            Task { await model.change(item, QueueChange(force: true)) }
        }
        Button("Edit…", systemImage: "pencil") { edit(item) }
        if index > 0 {
            Button("Move up", systemImage: "arrow.up") {
                Task { await model.change(item, QueueChange(position: index - 1)) }
            }
        }
        if index < count - 1 {
            Button("Move down", systemImage: "arrow.down") {
                Task { await model.change(item, QueueChange(position: index + 1)) }
            }
        }
        Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = item.text }
        Divider()
        Button("Drop", systemImage: "trash", role: .destructive) {
            Task { await model.drop(item) }
        }
    }
}
