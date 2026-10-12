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
        guard context.coordinator.shown !== text else { return }
        context.coordinator.shown = text
        view.attributedText = text
    }
}

/// Highlight colors code as the cockpit highlights it, by the classes of its
/// runs, which the server works out (cmd/internal/highlight); and diffs.
enum Highlight {
    static var font: UIFont {
        UIFont.monospacedSystemFont(ofSize: UIFont.preferredFont(forTextStyle: .footnote).pointSize, weight: .regular)
    }

    static var boldFont: UIFont {
        UIFont.monospacedSystemFont(ofSize: UIFont.preferredFont(forTextStyle: .footnote).pointSize, weight: .semibold)
    }

    static func plain(_ text: String) -> NSAttributedString {
        NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: UIColor.label])
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

    /// The color of a class of highlighting, as Pygments names them short.
    private static func color(for name: String) -> UIColor? {
        switch name {
        case "": return nil
        case "kt", "nc", "nn": return .systemTeal
        case "kc", "no", "m", "l", "py": return .systemIndigo
        case "nf": return .systemBlue
        case "nb": return .systemPurple
        case "nt", "ow": return .systemPink
        case "na", "nd", "cp", "se": return .systemOrange
        case "gd", "gr", "err": return .systemRed
        case "gi": return .systemGreen
        case "gh", "gu": return .systemBlue
        case "gp", "go": return .secondaryLabel
        default: break
        }
        if name.hasPrefix("k") { return .systemPink }
        if name.hasPrefix("s") { return .systemRed }
        if name.hasPrefix("c") { return .secondaryLabel }
        return nil
    }

    /// diff colors a unified diff: what it adds, what it removes, its hunks.
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
            var attributes: [NSAttributedString.Key: Any] = [.font: base, .foregroundColor: UIColor.label]
            if headers.contains(where: { line.hasPrefix($0) }) || line == "---" || line == "+++" {
                attributes[.font] = bold
                attributes[.foregroundColor] = UIColor.secondaryLabel
            } else if line.hasPrefix("@@") {
                attributes[.foregroundColor] = UIColor.systemBlue
                attributes[.backgroundColor] = UIColor.systemBlue.withAlphaComponent(0.08)
            } else if line.hasPrefix("+") {
                attributes[.backgroundColor] = UIColor.systemGreen.withAlphaComponent(0.16)
            } else if line.hasPrefix("-") {
                attributes[.backgroundColor] = UIColor.systemRed.withAlphaComponent(0.16)
            } else if line.hasPrefix("\\") {
                attributes[.foregroundColor] = UIColor.secondaryLabel
            }
            result.append(NSAttributedString(string: i < lines.count - 1 ? line + "\n" : line, attributes: attributes))
        }
        result.endEditing()
        return result
    }
}
