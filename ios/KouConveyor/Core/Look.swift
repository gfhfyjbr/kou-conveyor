import SwiftUI
import UIKit

/// Look is how the app looks beyond light and dark: its accent, the shade
/// of its grounds and how round its corners are — the choices the cockpits
/// offer (Settings in the web cockpit, /appearance in the terminal), kept
/// on this iPhone. The app's own look is the cockpits' orange on iOS's own
/// greys, with the corners it always had.
@Observable
final class Look: @unchecked Sendable {
    /// The look of the app. Only the interface changes it, on the main
    /// thread.
    static let shared = Look()

    /// The app's own accent, #ff5b1f in light and in dark alike.
    static let defaultAccent = "orange"
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
    private(set) var ground: Color
    private(set) var groupedGround: Color
    private(set) var surface: Color

    private static let accentKey = "look.accent", shadeKey = "look.shade", radiusKey = "look.radius"

    private init() {
        let defaults = UserDefaults.standard
        // Locals, as an observed property is not read before all are set.
        let accent = defaults.string(forKey: Look.accentKey).flatMap(LookColor.normalAccent) ?? Look.defaultAccent
        let shade = defaults.string(forKey: Look.shadeKey).flatMap(Look.validShade) ?? Look.systemShade
        let radius = (defaults.object(forKey: Look.radiusKey) as? Int).flatMap(Look.validRadius) ?? Look.defaultRadius
        let accentColors = Look.accentColors(accent)
        let grounds = Look.grounds(shade)
        self.accent = accent
        self.shade = shade
        self.radius = radius
        accentUIColor = accentColors.accent
        accentColor = Color(uiColor: accentColors.accent)
        accentInk = Color(uiColor: accentColors.ink)
        ground = Color(uiColor: grounds.ground)
        groupedGround = Color(uiColor: grounds.grouped)
        surface = Color(uiColor: grounds.surface)
    }

    /// cornerScale is how round the corners are against the app's own.
    var cornerScale: CGFloat { CGFloat(radius) / CGFloat(Self.defaultRadius) }

    var isDefault: Bool {
        accent == Self.defaultAccent && shade == Self.systemShade && radius == Self.defaultRadius
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
        let shade = LookColor.shades.first { $0.name == self.shade }?.label ?? "System"
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
        let grounds = Self.grounds(shade)
        ground = Color(uiColor: grounds.ground)
        groupedGround = Color(uiColor: grounds.grouped)
        surface = Color(uiColor: grounds.surface)
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
        choose(shade: Self.systemShade)
        choose(radius: Self.defaultRadius)
    }

    /// The app's own look is kept as no choice at all.
    private func save() {
        let defaults = UserDefaults.standard
        func keep(_ value: Any, _ key: String, unless own: Bool) {
            if own { defaults.removeObject(forKey: key) } else { defaults.set(value, forKey: key) }
        }
        keep(accent, Self.accentKey, unless: accent == Self.defaultAccent)
        keep(shade, Self.shadeKey, unless: shade == Self.systemShade)
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

    /// grounds are a screen's ground, a grouped list's, and a card's on
    /// them: iOS's own, or the cockpits' greys in the shade — bg-2, bg and
    /// bg-3 in light, bg, bg and bg-3 in dark, as iOS's are lighter and
    /// darker.
    static func grounds(_ shade: String) -> (ground: UIColor, grouped: UIColor, surface: UIColor) {
        guard let dark = LookColor.grounds(shade: shade, dark: true),
              let light = LookColor.grounds(shade: shade, dark: false)
        else {
            return (.systemBackground, .systemGroupedBackground, .secondarySystemBackground)
        }
        return (dynamic(light: light[1], dark: dark[0]), dynamic(light: light[0], dark: dark[0]),
                dynamic(light: light[2], dark: dark[2]))
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
}

extension Color {
    /// The accent: the cockpits' #ff5b1f, or the one chosen (Look).
    static var kouAccent: Color { Look.shared.accentColor }
    /// What is written on the accent: white, or near-black on a light one.
    static var kouAccentInk: Color { Look.shared.accentInk }
    /// A screen's ground: iOS's background, or the shade's.
    static var kouGround: Color { Look.shared.ground }
    /// A grouped list's ground: iOS's grouped background, or the shade's.
    static var kouGroupedGround: Color { Look.shared.groupedGround }
    /// A card's ground on a screen, such as code's.
    static var kouSurface: Color { Look.shared.surface }
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
