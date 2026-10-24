import SwiftUI
import UIKit

/// AppearanceScreen sets how the app looks: its accent, the shade of its
/// grounds and how round its corners are — the cockpits' choices (Settings
/// in the web cockpit, /appearance in the terminal), kept on this iPhone.
struct AppearanceScreen: View {
    /// The colour of one's own, as the picker holds it.
    @State private var custom = Color(uiColor: Look.uiColor(Look.shared.customHex ?? Look.shared.accentHex(dark: true)))
    private var look: Look { Look.shared }

    var body: some View {
        Form {
            // The preview is the row's card itself: a panel of its own inside
            // the row would be cut at its corners by the row's, rounder.
            Section {
                AppearancePreview()
                    .listRowInsets(EdgeInsets())
            }
            .kouRow()
            accentSection
            shadeSection
            cornersSection
            Section {
                Button {
                    withAnimation(.smooth) { look.reset() }
                } label: {
                    Label {
                        Text("Restore the app's own look")
                            .foregroundStyle(look.isDefault ? Color.kouFg4 : Color.kouFg)
                    } icon: {
                        RowIcon("arrow.uturn.backward", tone: look.isDefault ? .kouFg4 : .kouFg2)
                    }
                }
                .disabled(look.isDefault)
            } footer: {
                FormNote("Orange on the cockpits' graphite, with round corners.")
            }
            .kouRow()
        }
        .navigationTitle("Appearance")
        .navigationBarTitleDisplayMode(.inline)
        .groupedBackground()
        .listSectionSpacing(22)
        .sensoryFeedback(.selection, trigger: look.radius)
        .sensoryFeedback(.selection, trigger: look.accent)
        .sensoryFeedback(.selection, trigger: look.shade)
    }

    // MARK: Accent

    private var accentSection: some View {
        Section {
            // The presets in one row, as many columns as there are, up to 8.
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(minimum: 30), spacing: 4),
                                     count: min(max(LookColor.accents.count, 1), 8)),
                      spacing: 12) {
                ForEach(LookColor.accents, id: \.name) { preset in
                    swatch(preset)
                }
            }
            .padding(.vertical, 8)
            ColorPicker(selection: customColor, supportsOpacity: false) {
                LabeledContent("Colour of your own") {
                    if let hex = look.customHex {
                        Text(verbatim: hex.uppercased())
                            .font(.system(.subheadline, design: .monospaced))
                            .foregroundStyle(Color.kouFg2)
                    }
                }
            }
        } header: {
            HStack {
                Eyebrow("Accent")
                Spacer()
                Text(verbatim: accentName)
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouAccent)
                    .textCase(nil)
            }
        } footer: {
            FormNote("What runs, your prompts and the button that sends. A colour of your own is made lighter in dark and darker in light, as far as it takes to read.")
        }
        .kouRow()
    }

    /// The accent's name, or its hex when it is one's own.
    private var accentName: String {
        LookColor.accents.first { $0.name == look.accent }?.label ?? look.accent.uppercased()
    }

    private func swatch(_ preset: LookColor.Accent) -> some View {
        let selected = look.accent == preset.name
        let colors = Look.accentColors(preset.name)
        return Button {
            withAnimation(.smooth(duration: 0.25)) { look.choose(accent: preset.name) }
        } label: {
            Circle()
                .fill(Color(uiColor: colors.accent))
                .overlay(Circle().strokeBorder(Color.white.opacity(0.12), lineWidth: 1))
                .frame(width: 30, height: 30)
                .overlay {
                    Image(systemName: "checkmark")
                        .font(.caption.weight(.heavy))
                        .foregroundStyle(Color(uiColor: colors.ink))
                        .scaleEffect(selected ? 1 : 0.4)
                        .opacity(selected ? 1 : 0)
                }
                .padding(3)
                .overlay {
                    Circle()
                        .strokeBorder(selected ? Color(uiColor: colors.accent) : Color.clear, lineWidth: 1.5)
                }
                .frame(maxWidth: .infinity)
                .contentShape(.circle)
        }
        .buttonStyle(.borderless)
        .accessibilityLabel(preset.label)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    /// The picker's colour; one picked is the accent, as #rrggbb.
    private var customColor: Binding<Color> {
        Binding {
            custom
        } set: { color in
            custom = color
            var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
            guard UIColor(color).getRed(&r, green: &g, blue: &b, alpha: &a) else { return }
            look.choose(accent: LookColor.hex(LookColor.RGB(r: r, g: g, b: b)))
        }
    }

    // MARK: Shade

    private var shadeSection: some View {
        Section {
            ForEach(LookColor.shades, id: \.name) { shade in
                shadeRow(shade.name, label: shade.label)
            }
            shadeRow(Look.systemShade, label: "System", note: "iOS's own greys")
        } header: {
            Eyebrow("Shade")
        } footer: {
            FormNote("The grounds, the lines and the text of every screen: the cockpits' shades of grey, or iOS's own.")
        }
        .kouRow()
    }

    private func shadeRow(_ name: String, label: String, note: String? = nil) -> some View {
        let selected = look.shade == name
        let chip = Look.chip(name)
        let shape = RoundedRectangle(cornerRadius: .corner(6), style: .continuous)
        return Button {
            withAnimation(.smooth(duration: 0.25)) { look.choose(shade: name) }
        } label: {
            HStack(spacing: 12) {
                // The shade in light and in dark, with the accent on it.
                HStack(spacing: 0) {
                    Color(uiColor: chip.light)
                    Color(uiColor: chip.dark)
                }
                .overlay {
                    Circle()
                        .fill(Color.kouAccent)
                        .frame(width: 6, height: 6)
                        .opacity(selected ? 1 : 0)
                }
                .frame(width: 38, height: 24)
                .clipShape(shape)
                .overlay(shape.strokeBorder(selected ? Color.kouAccent.opacity(0.7) : Color.kouLine2, lineWidth: 1))
                VStack(alignment: .leading, spacing: 1) {
                    Text(verbatim: label)
                        .foregroundStyle(Color.kouFg)
                    if let note {
                        Text(verbatim: note)
                            .font(.kouMeta)
                            .foregroundStyle(Color.kouFg3)
                    }
                }
                Spacer(minLength: 8)
                Image(systemName: "checkmark")
                    .font(.footnote.weight(.bold))
                    .foregroundStyle(Color.kouAccent)
                    .opacity(selected ? 1 : 0)
            }
            .contentShape(.rect)
        }
        .tint(.primary)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    // MARK: Corners

    private var cornersSection: some View {
        Section {
            Picker("Corners", selection: Binding {
                look.radius
            } set: { radius in
                withAnimation(.smooth(duration: 0.25)) { look.choose(radius: radius) }
            }) {
                ForEach(Look.corners, id: \.radius) { corner in
                    Text(corner.label).tag(corner.radius)
                }
            }
            .pickerStyle(.segmented)
            .listRowSeparator(.hidden)
            HStack(spacing: 12) {
                // The corner itself, as round as it is set.
                CornerGlyph(radius: CGFloat(look.radius) / CGFloat(max(Look.maxRadius, 1)))
                    .stroke(Color.kouAccent, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                    .frame(width: 18, height: 18)
                    .animation(.smooth(duration: 0.25), value: look.radius)
                    .accessibilityHidden(true)
                Slider(value: Binding {
                    Double(look.radius)
                } set: {
                    look.choose(radius: Int($0.rounded()))
                }, in: 0...Double(Look.maxRadius), step: 1) {
                    Text("Radius")
                }
                Text(verbatim: "\(look.radius)pt")
                    .font(.system(.subheadline, design: .monospaced, weight: .medium))
                    .monospacedDigit()
                    .foregroundStyle(Color.kouFg2)
                    .contentTransition(.numericText(value: Double(look.radius)))
                    .frame(minWidth: 40, alignment: .trailing)
            }
        } header: {
            Eyebrow("Corners")
        } footer: {
            FormNote("How round the app's cards, prompts and chips are. Round is the app's own; Sharp squares them, as the cockpits draw them by default. Sheets and iOS's own controls keep their corners.")
        }
        .kouRow()
    }
}

/// CornerGlyph is a corner — the left edge up, round, and along the top —
/// rounded by a fraction of its size: 0 square, 1 a quarter circle.
private struct CornerGlyph: Shape {
    var radius: CGFloat

    var animatableData: CGFloat {
        get { radius }
        set { radius = newValue }
    }

    func path(in rect: CGRect) -> Path {
        let r = max(0, min(1, radius)) * min(rect.width, rect.height)
        var path = Path()
        path.move(to: CGPoint(x: rect.minX, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.minX, y: rect.minY + r))
        if r > 0 {
            path.addArc(center: CGPoint(x: rect.minX + r, y: rect.minY + r), radius: r,
                        startAngle: .degrees(180), endAngle: .degrees(270), clockwise: false)
        }
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.minY))
        return path
    }
}

/// AppearancePreview is a piece of a session in the look chosen, drawn as
/// the transcript draws it: a prompt, the answer, and a call running.
private struct AppearancePreview: View {
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let look = Look.shared
        let panel = RoundedRectangle(cornerRadius: .corner(12), style: .continuous)
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Eyebrow("Preview", color: .kouFg4)
                Spacer()
                Text(verbatim: "\(look.accentHex(dark: scheme == .dark)) · \(look.radius)pt")
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg4)
            }
            Text("Make the corners round")
                .font(.subheadline)
                .foregroundStyle(Color.kouFg)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background {
                    ZStack {
                        UserBubble.shape.fill(Color.kouAccent.opacity(0.14))
                        UserBubble.shape.strokeBorder(Color.kouAccent.opacity(0.38), lineWidth: 1)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .trailing)
            Text("Done: every card follows the setting.")
                .font(.subheadline)
                .foregroundStyle(Color.kouFg)
            HStack(spacing: 8) {
                LiveDot(size: 6)
                    .frame(width: 18)
                Text("EDIT")
                    .font(.kouEyebrow)
                    .tracking(0.9)
                    .foregroundStyle(Color.kouFg2)
                Text("Look.swift")
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(Color.kouFg)
                Spacer(minLength: 0)
                Text("+12 −4")
                    .font(.kouMeta)
                    .foregroundStyle(Color.kouFg3)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 10)
            .kouPanel(radius: 12)
            .runLight(true, in: panel)
            HStack(spacing: 8) {
                Tag("running", tone: .kouAccent)
                Tag("2 files", tone: .kouFg3)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("A preview of the look")
    }
}
