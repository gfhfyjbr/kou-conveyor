import SwiftUI
import UIKit

/// PairingView pairs the app with a relay: the cockpit's /tunnel shows a
/// code to scan and a link to paste.
struct PairingView: View {
    @Environment(AppModel.self) private var model
    @State private var scanning = false
    @State private var manual = false
    @State private var busy = false
    @State private var error: String?
    @Namespace private var glass

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 30) {
                    hero
                    steps
                    if let error {
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(Color.kouErr)
                            Text(verbatim: error)
                                .foregroundStyle(Color.kouFg)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .font(.callout)
                        .padding(14)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .kouPanel(radius: 14, fill: Color.kouErr.opacity(0.08), stroke: Color.kouErr.opacity(0.45))
                        .transition(.blurReplace)
                    }
                }
                .padding(.horizontal, 24)
                .padding(.top, 28)
                .padding(.bottom, 24)
                .frame(maxWidth: 560)
                .frame(maxWidth: .infinity)
            }
            .scrollBounceBehavior(.basedOnSize)
            .background(backdrop)
            .safeAreaBar(edge: .bottom) {
                actions
            }
            .sensoryFeedback(.error, trigger: error) { _, new in new != nil }
            .sheet(isPresented: $scanning) {
                ScannerSheet { link in
                    scanning = false
                    pair(link)
                }
            }
            .sheet(isPresented: $manual) {
                ManualPairingSheet { config in
                    manual = false
                    pair(config)
                }
            }
        }
    }

    /// The tunnel's orbit over the app's name: what the app is, before what
    /// to do.
    private var hero: some View {
        VStack(spacing: 22) {
            Orbit(color: .kouAccent, live: true, size: 172)
            VStack(spacing: 10) {
                Eyebrow("Pair this iPhone", color: .kouAccent)
                Text(verbatim: "kou-conveyor")
                    .font(.system(size: 34, weight: .heavy, design: .monospaced))
                    .foregroundStyle(Color.kouFg)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
                Text("Your cockpit, carried through its relay: the sessions, the chat, the canvases — here.")
                    .font(.body)
                    .foregroundStyle(Color.kouFg3)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    /// How to pair, as a track of three steps.
    private var steps: some View {
        VStack(alignment: .leading, spacing: 0) {
            SectionLabel("How to pair")
                .padding(.bottom, 16)
            step(1, "In the cockpit, run `/tunnel`. If no relay is set up yet, the agent sets one up on a host of yours.")
            step(2, "The tunnel's dialog shows a pairing code and a link.")
            step(3, "Scan the code here, or with the Camera app, or paste the link.", last: true)
        }
        .padding(18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .kouPanel(radius: 18)
    }

    private func step(_ number: Int, _ text: LocalizedStringKey, last: Bool = false) -> some View {
        HStack(alignment: .top, spacing: 14) {
            Text(verbatim: String(format: "%02d", number))
                .font(.system(.caption, design: .monospaced, weight: .bold))
                .foregroundStyle(Color.kouAccent)
                .frame(width: 30, height: 30)
                .background(Color.kouAccentSoft, in: .circle)
                .overlay(Circle().strokeBorder(Color.kouAccent.opacity(0.4), lineWidth: 1))
            Text(text)
                .font(.subheadline)
                .foregroundStyle(Color.kouFg2)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 5)
                .padding(.bottom, last ? 0 : 18)
        }
        // The track down to the next step.
        .background(alignment: .leading) {
            if !last {
                Rectangle()
                    .fill(Color.kouLine2)
                    .frame(width: 1)
                    .padding(.top, 34)
                    .padding(.bottom, 4)
                    .padding(.leading, 14.5)
            }
        }
    }

    private var actions: some View {
        GlassEffectContainer(spacing: 12) {
            VStack(spacing: 10) {
                Button {
                    error = nil
                    scanning = true
                } label: {
                    Label("Scan pairing code", systemImage: "qrcode.viewfinder")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.glassProminent)
                .controlSize(.large)
                .glassEffectID("scan", in: glass)

                // Side by side, of one width, while their words fit on a
                // line; else one above the other.
                ViewThatFits(in: .horizontal) {
                    EvenRow(spacing: 10) { otherWays }
                    VStack(spacing: 10) { otherWays }
                }
                .buttonStyle(.glass)
                .controlSize(.large)
            }
            .labelStyle(EvenSymbolLabelStyle())
        }
        .disabled(busy)
        .overlay {
            if busy {
                ProgressView()
                    .controlSize(.large)
                    .padding(18)
                    .glassEffect(.regular, in: .circle)
            }
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 8)
    }

    /// The ways to pair besides the camera: the link copied, or by hand.
    @ViewBuilder private var otherWays: some View {
        Button {
            paste()
        } label: {
            Label("Paste link", systemImage: "doc.on.clipboard")
                .lineLimit(1)
                .frame(maxWidth: .infinity)
        }
        .glassEffectID("paste", in: glass)
        Button {
            error = nil
            manual = true
        } label: {
            Label("Enter by hand", systemImage: "keyboard")
                .lineLimit(1)
                .frame(maxWidth: .infinity)
        }
        .glassEffectID("manual", in: glass)
    }

    /// The ground: the shade's, a field of dots fading out from the orbit,
    /// and the accent's glow behind it — a chart table, not a poster.
    private var backdrop: some View {
        ZStack {
            Color.kouGround
            DotField()
                .mask(RadialGradient(colors: [.black, .black.opacity(0.35), .clear],
                                     center: UnitPoint(x: 0.5, y: 0.24), startRadius: 0, endRadius: 520))
            RadialGradient(colors: [Color.kouAccent.opacity(0.2), .clear],
                           center: UnitPoint(x: 0.5, y: 0.24), startRadius: 0, endRadius: 340)
        }
        .ignoresSafeArea()
    }

    private func paste() {
        guard let text = UIPasteboard.general.string, !text.isEmpty else {
            withAnimation { error = "The clipboard has no link. In the cockpit, the tunnel's dialog copies it." }
            return
        }
        pair(text)
    }

    private func pair(_ link: String) {
        do {
            pair(try RelayConfig(link: link))
        } catch {
            withAnimation { self.error = error.localizedDescription }
        }
    }

    private func pair(_ config: RelayConfig) {
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                try await model.pair(config)
            } catch {
                withAnimation { self.error = error.localizedDescription }
            }
        }
    }
}

/// DotField is a grid of dots in the shade's stronger line: the pairing
/// screen's ground.
private struct DotField: View {
    var spacing: CGFloat = 22

    var body: some View {
        Canvas { context, size in
            let dot = Path(ellipseIn: CGRect(x: -0.8, y: -0.8, width: 1.6, height: 1.6))
            let shading = GraphicsContext.Shading.color(Color.kouLine2)
            var y = spacing / 2
            while y < size.height {
                var x = (size.width.truncatingRemainder(dividingBy: spacing)) / 2
                while x < size.width {
                    context.fill(dot.offsetBy(dx: x, dy: y), with: shading)
                    x += spacing
                }
                y += spacing
            }
        }
        .accessibilityHidden(true)
    }
}

/// EvenRow sets its views side by side, all as wide as the widest of them,
/// so that a row of buttons reads as one set. Its ideal width is what that
/// takes, so a ViewThatFits picks it only where every title fits on a line.
struct EvenRow: Layout {
    var spacing: CGFloat = 10

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        guard !subviews.isEmpty else { return .zero }
        let sizes = subviews.map { $0.sizeThatFits(.unspecified) }
        let widest = sizes.map(\.width).max() ?? 0
        let tallest = sizes.map(\.height).max() ?? 0
        let needed = widest * CGFloat(subviews.count) + spacing * CGFloat(subviews.count - 1)
        let offered = proposal.width.map { $0.isFinite ? $0 : needed } ?? needed
        return CGSize(width: max(offered, needed), height: tallest)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        guard !subviews.isEmpty else { return }
        let tallest = subviews.map { $0.sizeThatFits(.unspecified).height }.max() ?? 0
        let width = (bounds.width - spacing * CGFloat(subviews.count - 1)) / CGFloat(subviews.count)
        var x = bounds.minX
        for subview in subviews {
            subview.place(at: CGPoint(x: x, y: bounds.midY), anchor: .leading, proposal: ProposedViewSize(width: width, height: tallest))
            x += width + spacing
        }
    }
}

/// EvenSymbolLabelStyle sets a label's symbol in a square of one size, so
/// that buttons whose symbols differ in height come out as tall as each
/// other, and the titles of buttons one above the other line up.
struct EvenSymbolLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        EvenSymbolLabel(configuration: configuration)
    }
}

private struct EvenSymbolLabel: View {
    let configuration: LabelStyleConfiguration
    @ScaledMetric(relativeTo: .body) private var side: CGFloat = 22

    var body: some View {
        HStack(spacing: 8) {
            configuration.icon
                .frame(width: side, height: side)
            configuration.title
        }
    }
}

/// ManualPairingSheet takes a relay's address, token and fingerprint, as
/// `kou-conveyor-relay -print-config` prints them.
struct ManualPairingSheet: View {
    var done: (RelayConfig) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var url = ""
    @State private var token = ""
    @State private var fingerprint = ""
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("https://relay.example.com:8420", text: $url)
                        .font(.system(.body, design: .monospaced))
                        .keyboardType(.URL)
                        .textContentType(.URL)
                } header: {
                    Eyebrow("Relay")
                }
                .kouRow()
                Section {
                    SecureField("Token", text: $token)
                        .font(.system(.body, design: .monospaced))
                        .textContentType(.password)
                } header: {
                    Eyebrow("Token")
                } footer: {
                    Text("On the relay's host: \(Text(verbatim: "kou-conveyor-relay -print-config").font(.system(.footnote, design: .monospaced)).foregroundStyle(Color.kouFg2))")
                        .font(.footnote)
                        .foregroundStyle(Color.kouFg3)
                }
                .kouRow()
                Section {
                    TextField("SHA-256, 64 hex digits", text: $fingerprint, axis: .vertical)
                        .font(.system(.footnote, design: .monospaced))
                } header: {
                    Eyebrow("Certificate fingerprint")
                } footer: {
                    FormNote("For a relay with its own certificate (-tls self-signed, the default). Leave it empty for a certificate a CA signed, or for -tls off.")
                }
                .kouRow()
                if let error {
                    Section {
                        Label {
                            Text(verbatim: error)
                                .foregroundStyle(Color.kouErr)
                        } icon: {
                            RowIcon("exclamationmark.triangle.fill", tone: .kouErr)
                        }
                    }
                    .kouRow()
                }
            }
            .groupedBackground()
            .autocorrectionDisabled()
            .textInputAutocapitalization(.never)
            .navigationTitle("Enter by hand")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Pair", systemImage: "checkmark") {
                        do {
                            done(try RelayConfig(url: url, token: token, fingerprint: fingerprint))
                        } catch {
                            self.error = error.localizedDescription
                        }
                    }
                    .buttonStyle(.glassProminent)
                    .disabled(url.isEmpty || token.isEmpty)
                }
            }
        }
    }
}
