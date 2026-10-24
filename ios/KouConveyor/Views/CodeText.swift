import SwiftUI
import UIKit

/// CodeText shows a long text as UITextView lays it out — selectable, and
/// fast with thousands of lines — in the fixed-width font Highlight gives it.
struct CodeText: UIViewRepresentable {
    let text: NSAttributedString

    final class Coordinator {
        var shown: NSAttributedString?
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> UITextView {
        let view = UITextView()
        view.isEditable = false
        view.isSelectable = true
        view.backgroundColor = .clear
        view.textContainerInset = UIEdgeInsets(top: 14, left: 12, bottom: 28, right: 12)
        view.alwaysBounceVertical = true
        view.dataDetectorTypes = []
        return view
    }

    func updateUIView(_ view: UITextView, context: Context) {
        // The selection's handles in the accent, as the app's own text's.
        view.tintColor = UIColor(Color.kouAccent)
        guard context.coordinator.shown !== text else { return }
        context.coordinator.shown = text
        view.attributedText = text
    }
}

/// Highlight colors code as the cockpit highlights it, by the classes of its
/// runs, which the server works out (cmd/internal/highlight); and diffs.
enum Highlight {
    /// A text colored off the main thread, handed to it whole: a long file
    /// takes a while to color, and the screen would stop meanwhile. Nothing
    /// changes the text once it is made.
    struct Colored: @unchecked Sendable {
        let text: NSAttributedString
    }

    /// colored makes a text off the main thread, in the size of the text the
    /// main thread reads: away from it, the size the user chose is not known.
    @MainActor
    static func colored(_ make: @escaping @Sendable () -> NSAttributedString) async -> Colored {
        let size = textSize
        return await Task.detached(priority: .userInitiated) {
            $pointSize.withValue(size) { Colored(text: make()) }
        }.value
    }

    /// The size of the text colored away from the main thread.
    @TaskLocal private static var pointSize: CGFloat?

    private static var textSize: CGFloat {
        pointSize ?? UIFont.preferredFont(forTextStyle: .footnote).pointSize
    }

    static var font: UIFont {
        UIFont.monospacedSystemFont(ofSize: textSize, weight: .regular)
    }

    static var boldFont: UIFont {
        UIFont.monospacedSystemFont(ofSize: textSize, weight: .semibold)
    }

    static func plain(_ text: String) -> NSAttributedString {
        NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: CodeInk.fg])
    }

    /// attributed colors text by its runs: pairs of a length, in UTF-16 code
    /// units as NSString counts them, and the index of a class in classes.
    static func attributed(_ text: String, runs: [Int32]?, classes: [String]?) -> NSAttributedString {
        let result = NSMutableAttributedString(attributedString: plain(text))
        guard let runs, let classes, runs.count >= 2 else { return result }
        let total = result.length
        let bold = boldFont
        var at = 0
        var i = 0
        result.beginEditing()
        while i + 1 < runs.count, at < total {
            let length = min(Int(runs[i]), total - at)
            let index = Int(runs[i + 1])
            if length > 0, classes.indices.contains(index) {
                let name = classes[index]
                let range = NSRange(location: at, length: length)
                if let color = color(for: name) { result.addAttribute(.foregroundColor, value: color, range: range) }
                if ["gs", "gh", "gu"].contains(name) { result.addAttribute(.font, value: bold, range: range) }
            }
            at += max(length, 0)
            i += 2
        }
        result.endEditing()
        return result
    }

    /// The color of a class of highlighting, as Pygments names them short,
    /// in the cockpit's tokens (theme.css, .tk-*).
    static func color(for name: String) -> UIColor? {
        switch name {
        case "": return nil
        case "k", "kd", "kn", "kr", "kp", "ow", "nt", "gh": return CodeInk.keyword
        case "kt", "nc": return CodeInk.type
        case "kc", "no", "m", "mi", "mf", "mh", "mo", "l", "nd", "cp", "ss", "nl": return CodeInk.constant
        case "nb", "bp", "se", "si", "sr": return CodeInk.builtin
        case "nf", "fm", "na", "gu": return CodeInk.function
        case "sd", "c", "c1", "cm", "ch", "cs", "gp": return CodeInk.comment
        case "p": return CodeInk.punct
        case "o", "go": return CodeInk.op
        case "py", "nv", "vc", "vg", "vi", "nn": return CodeInk.prop
        case "gd", "gr", "err": return CodeInk.err
        case "gi": return CodeInk.ok
        default: break
        }
        // The classes the server may name longer: by their family.
        if name.hasPrefix("k") { return CodeInk.keyword }
        if name.hasPrefix("s") { return CodeInk.string }
        if name.hasPrefix("c") { return CodeInk.comment }
        if name.hasPrefix("m") { return CodeInk.constant }
        return nil
    }

    /// diff colors a unified diff as the cockpit's changes panel does: the
    /// lines it adds on the success's tint and those it removes on the
    /// failure's, their signs in those colours; its hunks on a faint band;
    /// the lines around them a step back.
    static func diff(_ patch: String) -> NSAttributedString {
        let result = NSMutableAttributedString()
        let base = font
        let bold = boldFont
        var lines = patch.components(separatedBy: "\n")
        if lines.last == "" { lines.removeLast() }
        let headers = ["diff ", "index ", "--- ", "+++ ", "new file", "deleted file", "similarity", "rename ", "old mode",
                       "new mode", "Binary files"]
        result.beginEditing()
        for (i, line) in lines.enumerated() {
            let end = i < lines.count - 1 ? "\n" : ""
            var attributes: [NSAttributedString.Key: Any] = [.font: base, .foregroundColor: CodeInk.op]
            var sign: UIColor?
            if headers.contains(where: { line.hasPrefix($0) }) || line == "---" || line == "+++" {
                attributes[.font] = bold
                attributes[.foregroundColor] = CodeInk.comment
            } else if line.hasPrefix("@@") {
                attributes[.foregroundColor] = CodeInk.comment
                attributes[.backgroundColor] = CodeInk.hunk
            } else if line.hasPrefix("+") {
                attributes[.foregroundColor] = CodeInk.fg
                attributes[.backgroundColor] = CodeInk.added
                sign = CodeInk.ok
            } else if line.hasPrefix("-") {
                attributes[.foregroundColor] = CodeInk.fg
                attributes[.backgroundColor] = CodeInk.removed
                sign = CodeInk.err
            } else if line.hasPrefix("\\") {
                attributes[.foregroundColor] = CodeInk.comment
            }
            if let sign {
                var signed = attributes
                signed[.foregroundColor] = sign
                result.append(NSAttributedString(string: String(line.prefix(1)), attributes: signed))
                result.append(NSAttributedString(string: String(line.dropFirst()) + end, attributes: attributes))
            } else {
                result.append(NSAttributedString(string: line + end, attributes: attributes))
            }
        }
        result.endEditing()
        return result
    }
}

/// CodeInk is the cockpit's colours of code (theme.css, --syn-*): the
/// tokens' own, whatever the accent, and greys of the graphite shade; in
/// light, darker. Constant, they colour code away from the main thread.
enum CodeInk {
    static let fg = Look.dynamic(light: "#1c1c19", dark: "#e3e2dc")
    static let keyword = Look.dynamic(light: "#c23d0a", dark: "#ff8a57")
    static let type = Look.dynamic(light: "#2d5f9a", dark: "#7aa7d9")
    static let constant = Look.dynamic(light: "#9c3d87", dark: "#d28bc4")
    static let builtin = Look.dynamic(light: "#1f7a72", dark: "#6fc2b8")
    static let function = Look.dynamic(light: "#8a5a00", dark: "#efc36e")
    static let string = Look.dynamic(light: "#2e7a34", dark: "#9ccf83")
    static let comment = Look.dynamic(light: "#8f8c83", dark: "#6d6b64")
    static let punct = Look.dynamic(light: "#6f6c64", dark: "#8c8a83")
    static let op = Look.dynamic(light: "#55534c", dark: "#b3b1a9")
    static let prop = Look.dynamic(light: "#3d3c38", dark: "#cfcdc5")
    static let err = Look.dynamic(light: "#cc1f3d", dark: "#ff5468")
    static let ok = Look.dynamic(light: "#2e7a34", dark: "#9ccf83")
    /// A diff's lines put in and taken out, and its hunks' band.
    static let added = tint(light: "#2e7a34", dark: "#9ccf83", alpha: 0.11)
    static let removed = tint(light: "#cc1f3d", dark: "#ff5468", alpha: 0.10)
    static let hunk = tint(light: "#1c1c19", dark: "#e3e2dc", alpha: 0.06)

    private static func tint(light: String, dark: String, alpha: CGFloat) -> UIColor {
        let light = Look.uiColor(light).withAlphaComponent(alpha), dark = Look.uiColor(dark).withAlphaComponent(alpha)
        return UIColor { $0.userInterfaceStyle == .dark ? dark : light }
    }
}
