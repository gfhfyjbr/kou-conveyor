import SwiftUI
import UIKit

/// Look is how the app looks beyond light and dark: its accent, the shade
/// of its grounds and how round its corners are — the choices the cockpits
/// offer (Settings in the web cockpit, /appearance in the terminal), kept
/// on this iPhone. The app's own look is the cockpits': their orange on
/// their warm graphite, with iOS's round corners.
@Observable
final class Look: @unchecked Sendable {
    /// The look of the app. Only the interface changes it, on the main
    /// thread.
    static let shared = Look()

    /// The app's own accent, #ff5b1f in light and in dark alike.
    static let defaultAccent = "orange"
    /// The app's own shade: the cockpits' graphite.
    static let defaultShade = "graphite"
    /// The shade that keeps iOS's own greys.
    static let systemShade = "system"
    /// The corners the app always had: every radius in it is scaled by the
    /// one chosen over this one, 0 squaring them.
    static let defaultRadius = 12
    static let maxRadius = 16
    /// The cockpits' corners, by name.
    static let corners: [(label: String, radius: Int)] = [("Sharp", 0), ("Soft", 4), ("Rounded", 8), ("Round", 12)]

    /// The accent: a preset's name (LookColor.accents) or #rrggbb.
    private(set) var accent: String
    /// The shade of the grounds: systemShade or a cockpit's shade.
    private(set) var shade: String
    /// The corners, 0 to maxRadius points.
    private(set) var radius: Int

    private(set) var accentColor: Color
    private(set) var accentInk: Color
    private(set) var accentUIColor: UIColor
    /// The greys of the shade, as SwiftUI draws them.
    private(set) var colors: Colors
    /// The same greys as UIKit takes them: the same instances until the
    /// shade changes, so a view of UIKit tells a colour changed by them.
    private(set) var uiPalette: Palette

    private static let accentKey = "look.accent", shadeKey = "look.shade", radiusKey = "look.radius"

    private init() {
        let defaults = UserDefaults.standard
        // Locals, as an observed property is not read before all are set.
        let accent = defaults.string(forKey: Look.accentKey).flatMap(LookColor.normalAccent) ?? Look.defaultAccent
        let shade = defaults.string(forKey: Look.shadeKey).flatMap(Look.validShade) ?? Look.defaultShade
        let radius = (defaults.object(forKey: Look.radiusKey) as? Int).flatMap(Look.validRadius) ?? Look.defaultRadius
        let accentColors = Look.accentColors(accent)
        self.accent = accent
        self.shade = shade
        self.radius = radius
        accentUIColor = accentColors.accent
        accentColor = Color(uiColor: accentColors.accent)
        accentInk = Color(uiColor: accentColors.ink)
        let palette = Look.palette(shade)
        uiPalette = palette
        colors = Colors(palette)
    }

    /// cornerScale is how round the corners are against the app's own.
    var cornerScale: CGFloat { CGFloat(radius) / CGFloat(Self.defaultRadius) }

    var isDefault: Bool {
        accent == Self.defaultAccent && shade == Self.defaultShade && radius == Self.defaultRadius
    }

    /// customHex is the accent when it is a colour of one's own.
    var customHex: String? { LookColor.parse(accent) != nil ? accent : nil }

    /// accentHex is the accent as drawn in dark, or else in light.
    func accentHex(dark: Bool) -> String {
        accent == Self.defaultAccent ? "#ff5b1f" : LookColor.accent(accent, dark: dark) ?? "#ff5b1f"
    }

    /// summary names the look in a few words: "Blue · Slate · Sharp".
    var summary: String {
        let accent = LookColor.accents.first { $0.name == self.accent }?.label ?? self.accent.uppercased()
        let shade = LookColor.shades.first { $0.name == self.shade }?.label ?? "iOS"
        let corners = Self.corners.first { $0.radius == radius }?.label ?? "\(radius) pt"
        return "\(accent) · \(shade) · \(corners)"
    }

    // MARK: Choosing

    func choose(accent: String) {
        guard let accent = LookColor.normalAccent(accent), accent != self.accent else { return }
        self.accent = accent
        let colors = Self.accentColors(accent)
        accentUIColor = colors.accent
        accentColor = Color(uiColor: colors.accent)
        accentInk = Color(uiColor: colors.ink)
        save()
    }

    func choose(shade: String) {
        guard let shade = Self.validShade(shade), shade != self.shade else { return }
        self.shade = shade
        let palette = Self.palette(shade)
        uiPalette = palette
        colors = Colors(palette)
        save()
    }

    func choose(radius: Int) {
        let radius = max(0, min(Self.maxRadius, radius))
        guard radius != self.radius else { return }
        self.radius = radius
        save()
    }

    /// reset goes back to the app's own look.
    func reset() {
        choose(accent: Self.defaultAccent)
        choose(shade: Self.defaultShade)
        choose(radius: Self.defaultRadius)
    }

    /// The app's own look is kept as no choice at all.
    private func save() {
        let defaults = UserDefaults.standard
        func keep(_ value: Any, _ key: String, unless own: Bool) {
            if own { defaults.removeObject(forKey: key) } else { defaults.set(value, forKey: key) }
        }
        keep(accent, Self.accentKey, unless: accent == Self.defaultAccent)
        keep(shade, Self.shadeKey, unless: shade == Self.defaultShade)
        keep(radius, Self.radiusKey, unless: radius == Self.defaultRadius)
    }

    private static func validShade(_ shade: String) -> String? {
        shade == systemShade || LookColor.shades.contains { $0.name == shade } ? shade : nil
    }

    private static func validRadius(_ radius: Int) -> Int? {
        (0...maxRadius).contains(radius) ? radius : nil
    }

    // MARK: Colours

    /// accentColors are an accent and what is written on it, in light and
    /// in dark. The app's own orange is the same in both; another accent is
    /// the cockpits', darker in light. The ink is white where it reads, as
    /// on the orange, and near-black on a light accent.
    static func accentColors(_ accent: String) -> (accent: UIColor, ink: UIColor) {
        if accent == defaultAccent {
            return (uiColor("#ff5b1f"), .white)
        }
        let dark = LookColor.accent(accent, dark: true) ?? "#ff5b1f"
        let light = LookColor.accent(accent, dark: false) ?? "#ff5b1f"
        func ink(_ hex: String) -> String { LookColor.contrast("#ffffff", hex) >= 3 ? "#ffffff" : "#0c0c0b" }
        return (dynamic(light: light, dark: dark), dynamic(light: ink(light), dark: ink(dark)))
    }

    /// Palette is a shade's greys, each in light and in dark.
    struct Palette {
        /// A screen's ground, a grouped list's, and code's or a field's on
        /// them.
        let ground, grouped, surface: UIColor
        /// An outlined panel's ground (a tool call, a prompt), a grouped
        /// row's, and a chip's.
        let panel, card, chip: UIColor
        /// The hairline and the stronger line.
        let line, line2: UIColor
        /// Text, from the strongest to the faintest.
        let fg, fg2, fg3, fg4: UIColor
    }

    /// Colors are a palette as SwiftUI draws it.
    struct Colors {
        let ground, grouped, surface, panel, card, chip, line, line2, fg, fg2, fg3, fg4: Color
        /// The composer's panel and the keys cut into it: the panel a step
        /// up from the screen in dark, the screen's own in light; its keys
        /// the other way round — darker than the panel in both.
        let dock, well: Color

        init(_ p: Palette) {
            let raised = p.surface, flat = p.ground
            dock = Color(uiColor: UIColor { $0.userInterfaceStyle == .dark ? raised.resolvedColor(with: $0) : flat.resolvedColor(with: $0) })
            well = Color(uiColor: UIColor { $0.userInterfaceStyle == .dark ? flat.resolvedColor(with: $0) : raised.resolvedColor(with: $0) })
            ground = Color(uiColor: p.ground)
            grouped = Color(uiColor: p.grouped)
            surface = Color(uiColor: p.surface)
            panel = Color(uiColor: p.panel)
            card = Color(uiColor: p.card)
            chip = Color(uiColor: p.chip)
            line = Color(uiColor: p.line)
            line2 = Color(uiColor: p.line2)
            fg = Color(uiColor: p.fg)
            fg2 = Color(uiColor: p.fg2)
            fg3 = Color(uiColor: p.fg3)
            fg4 = Color(uiColor: p.fg4)
        }
    }

    /// palette is iOS's own greys, or the cockpits' in the shade: a plain
    /// screen is bg-2 in light and bg in dark, as iOS's are lighter and
    /// darker; a grouped one bg, its rows raised from it.
    static func palette(_ shade: String) -> Palette {
        guard let dark = LookColor.grounds(shade: shade, dark: true),
              let light = LookColor.grounds(shade: shade, dark: false),
              let darkInk = LookColor.inks(shade: shade, dark: true),
              let lightInk = LookColor.inks(shade: shade, dark: false)
        else {
            return Palette(ground: .systemBackground, grouped: .systemGroupedBackground,
                           surface: .secondarySystemBackground, panel: .secondarySystemBackground,
                           card: .secondarySystemGroupedBackground, chip: dynamic(light: "#e5e5ea", dark: "#2c2c2e"),
                           line: .separator, line2: .opaqueSeparator,
                           fg: .label, fg2: .secondaryLabel, fg3: .tertiaryLabel, fg4: .quaternaryLabel)
        }
        func pair(_ l: String, _ d: String) -> UIColor { dynamic(light: l, dark: d) }
        return Palette(ground: pair(light[1], dark[0]), grouped: pair(light[0], dark[0]),
                       surface: pair(light[2], dark[2]), panel: pair(light[1], dark[1]),
                       card: pair(light[1], dark[2]), chip: pair(light[3], dark[3]),
                       line: pair(lightInk[0], darkInk[0]), line2: pair(lightInk[1], darkInk[1]),
                       fg: pair(lightInk[2], darkInk[2]), fg2: pair(lightInk[3], darkInk[3]),
                       fg3: pair(lightInk[4], darkInk[4]), fg4: pair(lightInk[5], darkInk[5]))
    }

    /// chip is a shade's raised grey in light and in dark, for a swatch.
    static func chip(_ shade: String) -> (light: UIColor, dark: UIColor) {
        guard let dark = LookColor.grounds(shade: shade, dark: true),
              let light = LookColor.grounds(shade: shade, dark: false)
        else {
            return (uiColor("#e5e5ea"), uiColor("#2c2c2e"))
        }
        return (uiColor(light[3]), uiColor(dark[3]))
    }

    static func dynamic(light: String, dark: String) -> UIColor {
        let light = uiColor(light), dark = uiColor(dark)
        return UIColor { $0.userInterfaceStyle == .dark ? dark : light }
    }

    static func uiColor(_ hex: String) -> UIColor {
        let c = LookColor.parse(hex) ?? LookColor.RGB(r: 1, g: 91 / 255, b: 31 / 255)
        return UIColor(red: c.r, green: c.g, blue: c.b, alpha: 1)
    }

    /// The cockpits' states, tuned for their grounds: in light darker.
    static let ok = Color(uiColor: dynamic(light: "#2e7a34", dark: "#9ccf83"))
    static let warn = Color(uiColor: dynamic(light: "#9a6200", dark: "#efb443"))
    static let err = Color(uiColor: dynamic(light: "#cc1f3d", dark: "#ff5468"))
}

extension Color {
    /// The accent: the cockpits' #ff5b1f, or the one chosen (Look).
    static var kouAccent: Color { Look.shared.accentColor }
    /// What is written on the accent: white, or near-black on a light one.
    static var kouAccentInk: Color { Look.shared.accentInk }
    /// The accent laid faintly over a ground, for a mark.
    static var kouAccentSoft: Color { Look.shared.accentColor.opacity(0.13) }
    /// A screen's ground: the shade's, or iOS's background.
    static var kouGround: Color { Look.shared.colors.ground }
    /// A grouped list's ground: the shade's, or iOS's grouped background.
    static var kouGroupedGround: Color { Look.shared.colors.grouped }
    /// A card's ground on a screen, such as code's.
    static var kouSurface: Color { Look.shared.colors.surface }
    /// An outlined panel's ground: a tool call's, a prompt's.
    static var kouPanel: Color { Look.shared.colors.panel }
    /// A grouped list's row.
    static var kouCard: Color { Look.shared.colors.card }
    /// A chip's ground, raised from a card.
    static var kouChip: Color { Look.shared.colors.chip }
    /// The composer's panel, and the keys cut into it.
    static var kouDock: Color { Look.shared.colors.dock }
    static var kouWell: Color { Look.shared.colors.well }
    /// The hairline between things, and the stronger line.
    static var kouLine: Color { Look.shared.colors.line }
    static var kouLine2: Color { Look.shared.colors.line2 }
    /// Text, from the strongest to the faintest.
    static var kouFg: Color { Look.shared.colors.fg }
    static var kouFg2: Color { Look.shared.colors.fg2 }
    static var kouFg3: Color { Look.shared.colors.fg3 }
    static var kouFg4: Color { Look.shared.colors.fg4 }
    /// The states: done, a warning, a failure.
    static var kouOK: Color { Look.ok }
    static var kouWarn: Color { Look.warn }
    static var kouErr: Color { Look.err }
}

extension CGFloat {
    /// corner is a radius of the app's own look made as round as chosen:
    /// 0 when the corners are sharp.
    static func corner(_ radius: CGFloat) -> CGFloat { radius * Look.shared.cornerScale }
}

/// CornerCapsule is a capsule as round as the corners chosen: a capsule in
/// the app's own look and rounder, a rectangle less round, then square.
struct CornerCapsule: Shape {
    var scale: CGFloat = Look.shared.cornerScale

    func path(in rect: CGRect) -> Path {
        if scale >= 1 {
            return Capsule().path(in: rect)
        }
        return RoundedRectangle(cornerRadius: min(rect.width, rect.height) / 2 * scale, style: .continuous).path(in: rect)
    }
}

extension Shape where Self == CornerCapsule {
    static var cornerCapsule: CornerCapsule { CornerCapsule() }
}

extension ButtonBorderShape {
    /// A small button's capsule, as round as the corners chosen.
    static var cornerCapsule: ButtonBorderShape {
        let scale = Look.shared.cornerScale
        return scale >= 1 ? .capsule : .roundedRectangle(radius: 17 * scale)
    }
}
