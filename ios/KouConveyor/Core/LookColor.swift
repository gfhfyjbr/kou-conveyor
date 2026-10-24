import Foundation

/// LookColor is the colour arithmetic of the cockpits' appearance — the
/// accents, the shades of the greys and how a colour of one's own is made
/// to read — as the cockpits do it (cmd/internal/cockpit/appearance.go and
/// the web cockpit's appearance.js): the same colours, to the byte, from
/// the same choices. The arithmetic is OKLCH's (Björn Ottosson's Oklab),
/// which keeps lightness as the eye sees it while hue and chroma change.
enum LookColor {
    /// An accent preset: the colour in dark and in light, the light one
    /// darker to read on a light ground.
    struct Accent: Sendable {
        let name, label, dark, light: String
    }

    /// A shade preset tints graphite's greys: each keeps its lightness, and
    /// takes the hue and graphite's chroma times dark or light.
    struct Shade: Sendable {
        let name, label: String
        let hue, dark, light: Double
    }

    static let accents: [Accent] = [
        Accent(name: "orange", label: "Orange", dark: "#ff5b1f", light: "#e4470c"),
        Accent(name: "amber", label: "Amber", dark: "#f5a524", light: "#a16207"),
        Accent(name: "green", label: "Green", dark: "#3ecf72", light: "#18853f"),
        Accent(name: "teal", label: "Teal", dark: "#22c3b0", light: "#0b7f72"),
        Accent(name: "blue", label: "Blue", dark: "#4c8dff", light: "#2160e0"),
        Accent(name: "violet", label: "Violet", dark: "#9d7bff", light: "#6d44e0"),
        Accent(name: "pink", label: "Pink", dark: "#ff5c9d", light: "#d0246c"),
        Accent(name: "mono", label: "Mono", dark: "#ecebe6", light: "#151513"),
    ]

    static let shades: [Shade] = [
        Shade(name: "graphite", label: "Graphite", hue: 0, dark: 1, light: 1),
        Shade(name: "neutral", label: "Neutral", hue: 0, dark: 0, light: 0),
        Shade(name: "slate", label: "Slate", hue: 255, dark: 2.2, light: 1),
        Shade(name: "midnight", label: "Midnight", hue: 264, dark: 5, light: 1.8),
        Shade(name: "sage", label: "Sage", hue: 165, dark: 2, light: 0.9),
        Shade(name: "mauve", label: "Mauve", hue: 305, dark: 2.2, light: 1),
        Shade(name: "mocha", label: "Mocha", hue: 55, dark: 3, light: 1.2),
    ]

    /// Graphite's grounds, in dark and in light: the page's, a panel's, a
    /// raised row's and a chip's (the cockpits' bg to bg-4).
    static let graphite = (dark: ["#0c0c0b", "#121211", "#191917", "#22221f"],
                           light: ["#f4f3ef", "#fbfaf7", "#ecebe5", "#e2e0d8"])

    /// Graphite's lines and text, in dark and in light: the hairline, the
    /// stronger line, and the text from the strongest to the faintest (the
    /// cockpits' line, line-2 and fg to fg-4).
    static let graphiteInk = (dark: ["#262623", "#373632", "#ecebe6", "#b3b1a9", "#7f7d76", "#5a5852"],
                              light: ["#dcdad2", "#c6c3b9", "#151513", "#46453f", "#77756c", "#a19e94"])

    /// grounds are a shade's bg to bg-4 in dark or in light, or nil for a
    /// shade no cockpit has.
    static func grounds(shade name: String, dark: Bool) -> [String]? {
        tinted(dark ? graphite.dark : graphite.light, shade: name, dark: dark)
    }

    /// inks are a shade's line, line-2 and fg to fg-4 in dark or in light,
    /// or nil for a shade no cockpit has: tinted as its grounds are.
    static func inks(shade name: String, dark: Bool) -> [String]? {
        tinted(dark ? graphiteInk.dark : graphiteInk.light, shade: name, dark: dark)
    }

    /// tinted are graphite's greys in a shade: each keeps its lightness and
    /// takes the shade's hue, at graphite's chroma times the shade's.
    private static func tinted(_ base: [String], shade name: String, dark: Bool) -> [String]? {
        guard let shade = shades.first(where: { $0.name == name }) else { return nil }
        if shade.name == "graphite" { return base }
        let k = dark ? shade.dark : shade.light
        return base.map { hex in
            let c = oklch(hex)
            return self.hex(l: c.l, c: c.c * k, h: shade.hue)
        }
    }

    /// accent is an accent preset's colour, or a colour of one's own made
    /// lighter in dark and darker in light until it reads on the cockpits'
    /// ground; nil for neither.
    static func accent(_ accent: String, dark: Bool) -> String? {
        if let preset = accents.first(where: { $0.name == accent }) {
            return dark ? preset.dark : preset.light
        }
        guard parse(accent) != nil else { return nil }
        return dark ? fit(accent, against: graphite.dark[0], contrast: 4.5, lighter: true)
            : fit(accent, against: graphite.light[0], contrast: 3, lighter: false)
    }

    /// normalAccent is an accent as kept: a preset's name, or a colour as
    /// #rrggbb in lower case; nil for neither.
    static func normalAccent(_ accent: String) -> String? {
        let accent = accent.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if accents.contains(where: { $0.name == accent }) || parse(accent) != nil { return accent }
        return nil
    }

    // MARK: Arithmetic

    struct RGB: Sendable, Equatable {
        var r, g, b: Double
    }

    struct LCH: Sendable {
        var l, c, h: Double
    }

    /// parse reads #rrggbb, in either case.
    static func parse(_ hex: String) -> RGB? {
        let digits = hex.utf8.dropFirst()
        guard hex.utf8.count == 7, hex.utf8.first == UInt8(ascii: "#"),
              digits.allSatisfy({ (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0) }),
              let v = UInt32(String(decoding: digits, as: UTF8.self), radix: 16)
        else { return nil }
        return RGB(r: Double(v >> 16 & 0xff) / 255, g: Double(v >> 8 & 0xff) / 255, b: Double(v & 0xff) / 255)
    }

    /// hex writes a colour as #rrggbb, each channel clamped to 0…1.
    static func hex(_ c: RGB) -> String {
        func byte(_ v: Double) -> Int { Int((max(0, min(1, v)) * 255).rounded()) }
        return String(format: "#%02x%02x%02x", byte(c.r), byte(c.g), byte(c.b))
    }

    static func toLinear(_ c: Double) -> Double {
        c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4)
    }

    static func fromLinear(_ c: Double) -> Double {
        let c = max(0, min(1, c))
        return c <= 0.0031308 ? 12.92 * c : 1.055 * pow(c, 1 / 2.4) - 0.055
    }

    static func oklch(_ hex: String) -> LCH {
        let rgb = parse(hex) ?? RGB(r: 0, g: 0, b: 0)
        let r = toLinear(rgb.r), g = toLinear(rgb.g), b = toLinear(rgb.b)
        let l = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
        let m = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
        let s = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
        let L = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s
        let A = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s
        let B = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s
        return LCH(l: L, c: hypot(A, B), h: fmod(atan2(B, A) * 180 / .pi + 360, 360))
    }

    /// linear is an OKLCH colour in linear sRGB, out of 0…1 when sRGB does
    /// not show it.
    static func linear(_ l: Double, _ c: Double, _ h: Double) -> RGB {
        let A = c * cos(h * .pi / 180), B = c * sin(h * .pi / 180)
        var l1 = l + 0.3963377774 * A + 0.2158037573 * B
        var m1 = l - 0.1055613458 * A - 0.0638541728 * B
        var s1 = l - 0.0894841775 * A - 1.2914855480 * B
        l1 = l1 * l1 * l1
        m1 = m1 * m1 * m1
        s1 = s1 * s1 * s1
        return RGB(r: 4.0767416621 * l1 - 3.3077115913 * m1 + 0.2309699292 * s1,
                   g: -1.2684380046 * l1 + 2.6097574011 * m1 - 0.3413193965 * s1,
                   b: -0.0041960863 * l1 - 0.7034186147 * m1 + 1.7076147010 * s1)
    }

    static func inGamut(_ l: Double, _ c: Double, _ h: Double) -> Bool {
        let v = linear(l, c, h)
        let e = 1e-4
        return v.r >= -e && v.r <= 1 + e && v.g >= -e && v.g <= 1 + e && v.b >= -e && v.b <= 1 + e
    }

    /// hex is an OKLCH colour as #rrggbb, its chroma reduced to what sRGB
    /// shows.
    static func hex(l: Double, c: Double, h: Double) -> String {
        var c = c
        if !inGamut(l, c, h) {
            var lo = 0.0, hi = c
            for _ in 0..<30 {
                let mid = (lo + hi) / 2
                if inGamut(l, mid, h) { lo = mid } else { hi = mid }
            }
            c = lo
        }
        let v = linear(l, c, h)
        return hex(RGB(r: fromLinear(v.r), g: fromLinear(v.g), b: fromLinear(v.b)))
    }

    static func luminance(_ hex: String) -> Double {
        let c = parse(hex) ?? RGB(r: 0, g: 0, b: 0)
        return 0.2126 * toLinear(c.r) + 0.7152 * toLinear(c.g) + 0.0722 * toLinear(c.b)
    }

    /// contrast is WCAG's contrast between two colours, 1 to 21.
    static func contrast(_ a: String, _ b: String) -> Double {
        let la = luminance(a), lb = luminance(b)
        return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
    }

    /// fit makes a colour lighter, or darker, keeping its hue and chroma,
    /// until it has the contrast needed against a ground.
    static func fit(_ hex: String, against ground: String, contrast need: Double, lighter: Bool) -> String {
        if contrast(hex, ground) >= need { return hex }
        let c = oklch(hex)
        var lo = 0.0, hi = c.l
        if lighter {
            lo = c.l
            hi = 1
        }
        for _ in 0..<40 {
            let mid = (lo + hi) / 2
            let ok = contrast(self.hex(l: mid, c: c.c, h: c.h), ground) >= need
            if ok == lighter { hi = mid } else { lo = mid }
        }
        return self.hex(l: lighter ? hi : lo, c: c.c, h: c.h)
    }
}
