import SwiftUI

/// PlanetMark is the tunnel's planet, as the cockpit draws it beside its
/// mark: a disc in a tilted ring, whose front crosses the disc.
struct PlanetMark: View {
    /// The width of its lines, in a 20-point square.
    var lineWidth: CGFloat = 1.6

    var body: some View {
        Canvas { context, size in
            let side = min(size.width, size.height)
            context.translateBy(x: (size.width - side) / 2, y: (size.height - side) / 2)
            context.scaleBy(x: side / 20, y: side / 20)
            let tilt = CGAffineTransform(translationX: 10, y: 10).rotated(by: -24 * .pi / 180).translatedBy(x: -10, y: -10)
            let ring = Path(ellipseIn: CGRect(x: 1, y: 6.9, width: 18, height: 6.2)).applying(tilt)
            let disc = Path(ellipseIn: CGRect(x: 5.3, y: 5.3, width: 9.4, height: 9.4))
            let style = StrokeStyle(lineWidth: lineWidth, lineCap: .round)
            // The back of the ring, hidden by the planet.
            var back = context
            back.clip(to: disc, options: .inverse)
            back.stroke(ring, with: .foreground, style: style)
            context.stroke(disc, with: .foreground, style: style)
            // The front of the ring: the half nearer, over the planet.
            var front = context
            front.clip(to: Path(CGRect(x: -6, y: 10, width: 32, height: 16)).applying(tilt))
            front.stroke(ring, with: .foreground, style: style)
        }
        .accessibilityHidden(true)
    }
}

/// How the tunnel is, as the app sees it.
enum TunnelState {
    case up, cockpitOffline, down, checking

    init(_ link: AppModel.Link) {
        switch link {
        case .online: self = .up
        case .cockpitOffline: self = .cockpitOffline
        case .failed: self = .down
        case .checking: self = .checking
        }
    }

    var color: Color {
        switch self {
        case .up: .kouAccent
        case .cockpitOffline: .yellow
        case .down: .red
        case .checking: .secondary
        }
    }

    var label: String {
        switch self {
        case .up: "Connected"
        case .cockpitOffline: "Cockpit offline"
        case .down: "Relay unreachable"
        case .checking: "Connecting"
        }
    }
}

/// TunnelChip is the cockpit's indicator: a rectangle with the planet in it
/// and "tunnelling".
struct TunnelChip: View {
    let state: TunnelState
    var showsWord = true

    var body: some View {
        HStack(spacing: 5) {
            PlanetMark()
                .frame(width: 15, height: 15)
            if showsWord {
                Text("tunnelling")
                    .font(.system(size: 12, weight: .semibold, design: .monospaced))
                    .fixedSize()
            }
        }
        .foregroundStyle(state.color)
        .padding(.horizontal, 7)
        .padding(.vertical, 4)
        .background(state.color.opacity(0.12), in: .rect(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(state.color.opacity(0.55), lineWidth: 1))
        .phaseAnimator(state == .up ? [1.0] : [1.0, 0.45]) { content, opacity in
            content.opacity(opacity)
        } animation: { _ in
            .easeInOut(duration: 0.8)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Tunnelling: \(state.label)")
    }
}

/// TunnelAccessory floats above the tab bar while the cockpit is shown:
/// the tunnel's chip, the relay, and how the cockpit behind it is.
struct TunnelAccessory: View {
    @Environment(AppModel.self) private var model
    @Environment(\.tabViewBottomAccessoryPlacement) private var placement

    var body: some View {
        let state = TunnelState(model.link)
        Button {
            model.tab = .relay
        } label: {
            HStack(spacing: 10) {
                TunnelChip(state: state)
                if placement != .inline {
                    VStack(alignment: .leading, spacing: 0) {
                        Text(model.relay?.address ?? "")
                            .font(.footnote.weight(.medium))
                            .lineLimit(1)
                        Text(detail(state))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 0)
                if placement != .inline, model.relay?.isEncrypted == true {
                    Image(systemName: "lock.fill")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 14)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
    }

    private func detail(_ state: TunnelState) -> String {
        switch state {
        case .up:
            if let workspace = model.workspace { return "\(state.label) · \(workspace.name)" }
            return state.label
        default:
            return state.label
        }
    }
}

#Preview("Chip") {
    VStack(spacing: 16) {
        TunnelChip(state: .up)
        TunnelChip(state: .cockpitOffline)
        TunnelChip(state: .down)
        PlanetMark(lineWidth: 1.2)
            .frame(width: 120, height: 120)
            .foregroundStyle(Color.kouAccent)
    }
    .padding()
}
