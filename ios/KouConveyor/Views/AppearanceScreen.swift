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
            Section {
                AppearancePreview()
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.kouGround)
            }
            accentSection
            shadeSection
            cornersSection
            Section {
                Button("Restore the app's own look", systemImage: "arrow.uturn.backward") {
                    withAnimation(.smooth) { look.reset() }
                }
                .disabled(look.isDefault)
            } footer: {
                Text("Orange on iOS's own greys, with round corners.")
            }
        }
        .navigationTitle("Appearance")
        .navigationBarTitleDisplayMode(.inline)
        .groupedBackground()
        .sensoryFeedback(.selection, trigger: look.radius)
    }

    // MARK: Accent

    private var accentSection: some View {
        Section {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 40, maximum: 52), spacing: 10)], spacing: 12) {
                ForEach(LookColor.accents, id: \.name) { preset in
                    swatch(preset)
                }
            }
            .padding(.vertical, 6)
            ColorPicker(selection: customColor, supportsOpacity: false) {
                LabeledContent("Colour of your own") {
                    if let hex = look.customHex {
                        Text(hex.uppercased())
                            .font(.system(.body, design: .monospaced))
                    }
                }
            }
        } header: {
            Text("Accent")
        } footer: {
            Text("Buttons, your prompts and what runs. A colour of your own is made lighter in dark and darker in light, as far as it takes to read.")
        }
    }

    private func swatch(_ preset: LookColor.Accent) -> some View {
        let selected = look.accent == preset.name
        return Button {
            withAnimation(.smooth(duration: 0.25)) { look.choose(accent: preset.name) }
        } label: {
            Circle()
                .fill(Color(uiColor: Look.accentColors(preset.name).accent).gradient)
                .frame(width: 34, height: 34)
                .overlay {
                    if selected {
                        Image(systemName: "checkmark")
                            .font(.footnote.weight(.bold))
                            .foregroundStyle(Color(uiColor: Look.accentColors(preset.name).ink))
                    }
                }
                .padding(4)
                .overlay {
                    Circle()
                        .strokeBorder(selected ? Color.primary.opacity(0.85) : .clear, lineWidth: 2)
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
            shadeRow(Look.systemShade, label: "System")
            ForEach(LookColor.shades, id: \.name) { shade in
                shadeRow(shade.name, label: shade.label)
            }
        } header: {
            Text("Shade")
        } footer: {
            Text("The ground of the transcript, of lists and of code: iOS's own greys, or the cockpits' shades of them. Rows and sheets keep iOS's.")
        }
    }

    private func shadeRow(_ name: String, label: String) -> some View {
        let selected = look.shade == name
        let chip = Look.chip(name)
        return Button {
            withAnimation(.smooth(duration: 0.25)) { look.choose(shade: name) }
        } label: {
            HStack(spacing: 12) {
                HStack(spacing: 0) {
                    Color(uiColor: chip.light)
                    Color(uiColor: chip.dark)
                }
                .frame(width: 34, height: 22)
                .clipShape(.rect(cornerRadius: .corner(6)))
                .overlay(RoundedRectangle(cornerRadius: .corner(6)).strokeBorder(.separator, lineWidth: 0.5))
                Text(label)
                Spacer(minLength: 8)
                Image(systemName: "checkmark")
                    .fontWeight(.semibold)
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
                Image(systemName: "square")
                    .foregroundStyle(.secondary)
                Slider(value: Binding {
                    Double(look.radius)
                } set: {
                    look.choose(radius: Int($0.rounded()))
                }, in: 0...Double(Look.maxRadius), step: 1) {
                    Text("Radius")
                }
                Image(systemName: "app")
                    .foregroundStyle(.secondary)
                Text("\(look.radius) pt")
                    .font(.subheadline.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .frame(minWidth: 40, alignment: .trailing)
            }
        } header: {
            Text("Corners")
        } footer: {
            Text("How round the app's cards, bubbles and chips are. Round is the app's own; Sharp squares them, as the cockpits draw them by default. Lists, sheets and iOS's own controls keep their corners.")
        }
    }
}

/// AppearancePreview is a piece of a session in the look chosen: a prompt,
/// the answer with its code, and a run going on.
private struct AppearancePreview: View {
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let look = Look.shared
        VStack(alignment: .leading, spacing: 12) {
            Text("Make the corners round")
                .font(.subheadline)
                .foregroundStyle(Color.kouAccentInk)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(Color.kouAccent.gradient, in: .rect(cornerRadius: .corner(20)))
                .frame(maxWidth: .infinity, alignment: .trailing)
            Text("Done: every card follows the setting.")
                .font(.subheadline)
            Text(verbatim: "accent \(look.accentHex(dark: scheme == .dark)) · radius \(look.radius)pt")
                .font(.system(.caption, design: .monospaced))
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.kouSurface, in: .rect(cornerRadius: .corner(12)))
                .overlay(RoundedRectangle(cornerRadius: .corner(12)).strokeBorder(.separator.opacity(0.6), lineWidth: 0.5))
            HStack(spacing: 8) {
                Label("Working", systemImage: "bolt.fill")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(Color.kouAccent)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .background(Color.kouAccent.opacity(0.15), in: .cornerCapsule)
                Text("2 files changed")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity)
        .background(Color.kouGround)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("A preview of the look")
    }
}
