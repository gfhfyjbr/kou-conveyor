import Foundation

// A surface of the genui plugin (examples/plugins/genui): its data model as
// the plugin's ui.py makes it and the web cockpit's data.js reads it — JSON
// Pointers, the sources' formats, templates, numbers and a Metric's change —
// so that a surface reads the same here as in the cockpit and in the text the
// agent got back. What is here mirrors data.js; a change to one is a change
// to the other.

/// A JSON value of a surface, its objects' members in their order: a table
/// with no columns named shows its rows' fields as they come, and an event's
/// data reaches the agent as the surface wrote it.
enum SurfaceValue: Sendable, Equatable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([SurfaceValue])
    case object(SurfaceObject)

    var object: SurfaceObject? { if case let .object(o) = self { o } else { nil } }
    var array: [SurfaceValue]? { if case let .array(a) = self { a } else { nil } }
    var string: String? { if case let .string(s) = self { s } else { nil } }
    var number: Double? { if case let .number(n) = self { n } else { nil } }
    var bool: Bool? { if case let .bool(b) = self { b } else { nil } }
    var isNull: Bool { self == .null }

    /// Whether a value is so in JavaScript: what a Toggle shows checked.
    var truthy: Bool {
        switch self {
        case .null: false
        case let .bool(b): b
        case let .number(n): n != 0 && !n.isNaN
        case let .string(s): !s.isEmpty
        case .array, .object: true
        }
    }
}

/// A JSON object of a surface: its members in the order they came.
struct SurfaceObject: Sendable, Equatable {
    private(set) var keys: [String] = []
    private var members: [String: SurfaceValue] = [:]

    init() {}

    init(_ pairs: [(String, SurfaceValue)]) {
        for (key, value) in pairs { self[key] = value }
    }

    subscript(key: String) -> SurfaceValue? {
        get { members[key] }
        set {
            if let newValue {
                if members.updateValue(newValue, forKey: key) == nil { keys.append(key) }
            } else if members.removeValue(forKey: key) != nil {
                keys.removeAll { $0 == key }
            }
        }
    }

    var isEmpty: Bool { keys.isEmpty }
    var count: Int { keys.count }
    var pairs: [(key: String, value: SurfaceValue)] { keys.compactMap { key in members[key].map { (key, $0) } } }

    func string(_ key: String) -> String? { self[key]?.string }
    func object(_ key: String) -> SurfaceObject? { self[key]?.object }
    func array(_ key: String) -> [SurfaceValue]? { self[key]?.array }
    func bool(_ key: String) -> Bool? { self[key]?.bool }
    /// A number member, or a string that reads as one.
    func number(_ key: String) -> Double? { self[key].flatMap(SurfaceText.asNumber) }
    /// A member that is a whole number.
    func int(_ key: String) -> Int? {
        guard let n = self[key]?.number, n.isFinite, n == n.rounded(), abs(n) < 1e15 else { return nil }
        return Int(n)
    }
}

// MARK: - JSON

struct SurfaceJSONError: Error, CustomStringConvertible {
    let description: String
}

enum SurfaceJSON {
    static func parse(_ text: String) throws -> SurfaceValue {
        var parser = SurfaceJSONParser(bytes: Array(text.utf8))
        parser.skip()
        let value = try parser.value(depth: 0)
        parser.skip()
        guard parser.at == parser.bytes.count else { throw parser.fail("unexpected text after the value") }
        return value
    }

    /// encode writes a value as JavaScript's JSON.stringify does: compact.
    static func encode(_ value: SurfaceValue) -> String {
        var out = ""
        write(value, into: &out)
        return out
    }

    private static func write(_ value: SurfaceValue, into out: inout String) {
        switch value {
        case .null: out += "null"
        case let .bool(b): out += b ? "true" : "false"
        case let .number(n): out += number(n)
        case let .string(s): quote(s, into: &out)
        case let .array(list):
            out += "["
            for (i, item) in list.enumerated() {
                if i > 0 { out += "," }
                write(item, into: &out)
            }
            out += "]"
        case let .object(o):
            out += "{"
            for (i, pair) in o.pairs.enumerated() {
                if i > 0 { out += "," }
                quote(pair.key, into: &out)
                out += ":"
                write(pair.value, into: &out)
            }
            out += "}"
        }
    }

    /// number is a number as JavaScript writes it: 3, 41.6, 1e-7.
    static func number(_ n: Double) -> String {
        guard n.isFinite else { return "null" }
        if n == n.rounded(), abs(n) < 1e21 {
            if abs(n) < 9.007_199_254_740_992e15 { return String(Int64(n)) }
            return String(format: "%.0f", n)
        }
        var text = "\(n)"
        if let e = text.firstIndex(where: { $0 == "e" || $0 == "E" }) {
            let mantissa = text[..<e]
            var exponent = String(text[text.index(after: e)...])
            var sign = "+"
            if exponent.hasPrefix("-") {
                sign = "-"
                exponent.removeFirst()
            } else if exponent.hasPrefix("+") {
                exponent.removeFirst()
            }
            while exponent.count > 1, exponent.hasPrefix("0") { exponent.removeFirst() }
            text = mantissa + "e" + sign + exponent
        }
        return text
    }

    static func quote(_ s: String, into out: inout String) {
        out += "\""
        for scalar in s.unicodeScalars {
            switch scalar {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            case "\u{08}": out += "\\b"
            case "\u{0C}": out += "\\f"
            default:
                if scalar.value < 0x20 {
                    out += String(format: "\\u%04x", scalar.value)
                } else {
                    out.unicodeScalars.append(scalar)
                }
            }
        }
        out += "\""
    }
}

/// A JSON parser that keeps the order of an object's members.
private struct SurfaceJSONParser {
    let bytes: [UInt8]
    var at = 0

    func fail(_ message: String) -> SurfaceJSONError {
        SurfaceJSONError(description: "\(message) at character \(at + 1)")
    }

    mutating func skip() {
        while at < bytes.count, bytes[at] == 0x20 || bytes[at] == 0x0A || bytes[at] == 0x0D || bytes[at] == 0x09 {
            at += 1
        }
    }

    mutating func value(depth: Int) throws -> SurfaceValue {
        guard depth < 256 else { throw fail("nested too deep") }
        guard at < bytes.count else { throw fail("unexpected end") }
        switch bytes[at] {
        case UInt8(ascii: "{"):
            at += 1
            var object = SurfaceObject()
            skip()
            if at < bytes.count, bytes[at] == UInt8(ascii: "}") {
                at += 1
                return .object(object)
            }
            while true {
                skip()
                guard at < bytes.count, bytes[at] == UInt8(ascii: "\"") else { throw fail("expected a name") }
                let key = try string()
                skip()
                guard at < bytes.count, bytes[at] == UInt8(ascii: ":") else { throw fail("expected ':'") }
                at += 1
                skip()
                object[key] = try value(depth: depth + 1)
                skip()
                guard at < bytes.count else { throw fail("unexpected end") }
                if bytes[at] == UInt8(ascii: ",") {
                    at += 1
                    continue
                }
                guard bytes[at] == UInt8(ascii: "}") else { throw fail("expected ',' or '}'") }
                at += 1
                return .object(object)
            }
        case UInt8(ascii: "["):
            at += 1
            var list: [SurfaceValue] = []
            skip()
            if at < bytes.count, bytes[at] == UInt8(ascii: "]") {
                at += 1
                return .array(list)
            }
            while true {
                skip()
                list.append(try value(depth: depth + 1))
                skip()
                guard at < bytes.count else { throw fail("unexpected end") }
                if bytes[at] == UInt8(ascii: ",") {
                    at += 1
                    continue
                }
                guard bytes[at] == UInt8(ascii: "]") else { throw fail("expected ',' or ']'") }
                at += 1
                return .array(list)
            }
        case UInt8(ascii: "\""):
            return .string(try string())
        case UInt8(ascii: "t"):
            try word("true")
            return .bool(true)
        case UInt8(ascii: "f"):
            try word("false")
            return .bool(false)
        case UInt8(ascii: "n"):
            try word("null")
            return .null
        default:
            return .number(try number())
        }
    }

    mutating func word(_ word: StaticString) throws {
        let count = word.utf8CodeUnitCount
        guard at + count <= bytes.count else { throw fail("unexpected end") }
        let expected = UnsafeBufferPointer(start: word.utf8Start, count: count)
        for i in 0..<count where bytes[at + i] != expected[i] {
            throw fail("unexpected character")
        }
        at += count
    }

    mutating func number() throws -> Double {
        let start = at
        if at < bytes.count, bytes[at] == UInt8(ascii: "-") { at += 1 }
        func digits(_ p: inout SurfaceJSONParser) -> Int {
            let from = p.at
            while p.at < p.bytes.count, (0x30...0x39).contains(p.bytes[p.at]) { p.at += 1 }
            return p.at - from
        }
        guard at < bytes.count, (0x30...0x39).contains(bytes[at]) else { throw fail("unexpected character") }
        if bytes[at] == 0x30 { at += 1 } else { _ = digits(&self) }
        if at < bytes.count, bytes[at] == UInt8(ascii: ".") {
            at += 1
            guard digits(&self) > 0 else { throw fail("a number without its fraction") }
        }
        if at < bytes.count, bytes[at] == UInt8(ascii: "e") || bytes[at] == UInt8(ascii: "E") {
            at += 1
            if at < bytes.count, bytes[at] == UInt8(ascii: "+") || bytes[at] == UInt8(ascii: "-") { at += 1 }
            guard digits(&self) > 0 else { throw fail("a number without its exponent") }
        }
        let text = String(decoding: bytes[start..<at], as: UTF8.self)
        guard let n = Double(text) else { throw fail("not a number") }
        return n
    }

    mutating func string() throws -> String {
        at += 1
        let start = at
        // Most strings have no escapes: they are taken as they are.
        while at < bytes.count {
            let c = bytes[at]
            if c == UInt8(ascii: "\"") {
                let text = String(decoding: bytes[start..<at], as: UTF8.self)
                at += 1
                return text
            }
            if c == UInt8(ascii: "\\") { break }
            if c < 0x20 { throw fail("a control character in a string") }
            at += 1
        }
        var out = Array(bytes[start..<at])
        while true {
            guard at < bytes.count else { throw fail("a string without its end") }
            let c = bytes[at]
            if c == UInt8(ascii: "\"") {
                at += 1
                return String(decoding: out, as: UTF8.self)
            }
            if c < 0x20 { throw fail("a control character in a string") }
            guard c == UInt8(ascii: "\\") else {
                out.append(c)
                at += 1
                continue
            }
            at += 1
            guard at < bytes.count else { throw fail("a string without its end") }
            let e = bytes[at]
            at += 1
            switch e {
            case UInt8(ascii: "\""): out.append(0x22)
            case UInt8(ascii: "\\"): out.append(0x5C)
            case UInt8(ascii: "/"): out.append(0x2F)
            case UInt8(ascii: "b"): out.append(0x08)
            case UInt8(ascii: "f"): out.append(0x0C)
            case UInt8(ascii: "n"): out.append(0x0A)
            case UInt8(ascii: "r"): out.append(0x0D)
            case UInt8(ascii: "t"): out.append(0x09)
            case UInt8(ascii: "u"):
                var unit = try hex4()
                var scalar: Unicode.Scalar?
                if (0xD800...0xDBFF).contains(unit), at + 1 < bytes.count, bytes[at] == UInt8(ascii: "\\"), bytes[at + 1] == UInt8(ascii: "u") {
                    let mark = at
                    at += 2
                    let low = try hex4()
                    if (0xDC00...0xDFFF).contains(low) {
                        scalar = Unicode.Scalar(0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00))
                    } else {
                        at = mark
                        unit = 0xFFFD
                    }
                }
                let s = scalar ?? Unicode.Scalar(unit) ?? "\u{FFFD}"
                out.append(contentsOf: Array(String(Character(s)).utf8))
            default:
                throw fail("an unknown escape")
            }
        }
    }

    mutating func hex4() throws -> UInt32 {
        guard at + 4 <= bytes.count else { throw fail("unexpected end") }
        var value: UInt32 = 0
        for _ in 0..<4 {
            let c = bytes[at]
            let digit: UInt32
            switch c {
            case 0x30...0x39: digit = UInt32(c - 0x30)
            case 0x41...0x46: digit = UInt32(c - 0x41 + 10)
            case 0x61...0x66: digit = UInt32(c - 0x61 + 10)
            default: throw fail("not a hex digit")
            }
            value = value * 16 + digit
            at += 1
        }
        return value
    }
}

// MARK: - JSON Pointer

enum SurfacePointer {
    static func tokens(_ pointer: String) -> [String]? {
        if pointer.isEmpty { return [] }
        guard pointer.hasPrefix("/") else { return nil }
        return pointer.dropFirst().split(separator: "/", omittingEmptySubsequences: false).map {
            $0.replacingOccurrences(of: "~1", with: "/").replacingOccurrences(of: "~0", with: "~")
        }
    }

    /// get finds what pointer names in doc; nil when it names nothing. A
    /// negative index counts from the end of a list: -1 is its last element.
    static func get(_ doc: SurfaceValue, _ pointer: String) -> SurfaceValue? {
        guard let list = tokens(pointer) else { return nil }
        var current = doc
        for token in list {
            switch current {
            case let .array(items):
                guard isIndex(token, negative: true), var i = Int(token) else { return nil }
                if i < 0 { i += items.count }
                guard items.indices.contains(i) else { return nil }
                current = items[i]
            case let .object(o):
                guard let next = o[token] else { return nil }
                current = next
            default:
                return nil
            }
        }
        return current
    }

    /// set sets (or with nil or null removes) what pointer names in doc,
    /// making the objects on the way.
    static func set(_ doc: inout SurfaceObject, _ pointer: String, _ value: SurfaceValue?) {
        guard let list = tokens(pointer), !list.isEmpty else { return }
        var root = SurfaceValue.object(doc)
        set(&root, list[...], value == .null ? nil : value)
        if let o = root.object { doc = o }
    }

    private static func set(_ current: inout SurfaceValue, _ list: ArraySlice<String>, _ value: SurfaceValue?) {
        guard let token = list.first else { return }
        let last = list.count == 1
        switch current {
        case var .array(items):
            if last {
                if token == "-", let value { items.append(value) }
                if isIndex(token, negative: false), let i = Int(token), items.indices.contains(i) {
                    if let value { items[i] = value } else { items.remove(at: i) }
                }
            } else {
                guard isIndex(token, negative: false), let i = Int(token), items.indices.contains(i) else { return }
                set(&items[i], list.dropFirst(), value)
            }
            current = .array(items)
        case var .object(o):
            if last {
                o[token] = value
            } else {
                var next = o[token] ?? .null
                switch next {
                case .object, .array: break
                default: next = .object(SurfaceObject())
                }
                set(&next, list.dropFirst(), value)
                o[token] = next
            }
            current = .object(o)
        default:
            return
        }
    }

    private static func isIndex(_ token: String, negative: Bool) -> Bool {
        var digits = Substring(token)
        if negative, digits.hasPrefix("-") { digits = digits.dropFirst() }
        return !digits.isEmpty && digits.allSatisfy { $0.isASCII && $0.isNumber }
    }

    /// binding is the pointer of a binding, { "path": "/…" }.
    static func binding(_ value: SurfaceValue?) -> String? {
        guard let o = value?.object, o.count == 1, let path = o.string("path") else { return nil }
        return path
    }

    /// resolve is a prop's value: what its binding finds, or itself; nil
    /// when a binding finds nothing.
    static func resolve(_ doc: SurfaceValue, _ value: SurfaceValue?) -> SurfaceValue? {
        guard let value else { return nil }
        if let path = binding(value) { return get(doc, path) }
        return value
    }
}

// MARK: - Sources

enum SurfaceSource {
    static let maxRows = 10_000
    static let maxText = 64 << 10

    static func format(of path: String) -> String {
        let ext = (path as NSString).pathExtension.lowercased()
        return ["json": "json", "jsonl": "jsonl", "ndjson": "jsonl", "csv": "csv", "tsv": "tsv", "md": "markdown",
                "markdown": "markdown"][ext] ?? "text"
    }

    /// parse is what a source's text is in the data model; it throws when
    /// the text is not of its format.
    static func parse(_ format: String, _ text: String) throws -> SurfaceValue {
        switch format {
        case "json":
            return try SurfaceJSON.parse(text)
        case "jsonl":
            var rows: [SurfaceValue] = []
            // Lines end where Python's splitlines ends them.
            let ends: Set<UInt32> = [0x0A, 0x0D, 0x0B, 0x0C, 0x1C, 0x1D, 0x1E, 0x85, 0x2028, 0x2029]
            for raw in text.unicodeScalars.split(whereSeparator: { ends.contains($0.value) }) {
                let line = String(raw).trimmingCharacters(in: .whitespaces)
                guard !line.isEmpty, let row = try? SurfaceJSON.parse(line) else { continue }
                rows.append(row)
            }
            let count = rows.count
            if rows.count > maxRows { rows = Array(rows.suffix(maxRows)) }
            return .object(SurfaceObject([("rows", .array(rows)), ("last", rows.last ?? .null), ("count", .number(Double(count)))]))
        case "csv", "tsv":
            let table = csvRows(text, delimiter: format == "csv" ? "," : "\t")
            guard let head = table.first else {
                return .object(SurfaceObject([("columns", .array([])), ("rows", .array([])), ("last", .null), ("count", .number(0))]))
            }
            let columns = head.map { $0.trimmingCharacters(in: .whitespaces) }
            var rows: [SurfaceValue] = []
            for r in table.dropFirst() {
                guard r.contains(where: { !$0.trimmingCharacters(in: .whitespaces).isEmpty }) else { continue }
                var row = SurfaceObject()
                for (i, c) in r.enumerated() {
                    row[i < columns.count && !columns[i].isEmpty ? columns[i] : "col\(i + 1)"] = cell(c)
                }
                rows.append(.object(row))
            }
            let count = rows.count
            if rows.count > maxRows { rows = Array(rows.suffix(maxRows)) }
            return .object(SurfaceObject([("columns", .array(columns.map(SurfaceValue.string))), ("rows", .array(rows)),
                                          ("last", rows.last ?? .null), ("count", .number(Double(count)))]))
        default:
            return .string(String(text.prefix(maxText)))
        }
    }

    private static func cell(_ text: String) -> SurfaceValue {
        let t = text.trimmingCharacters(in: .whitespaces)
        if SurfaceText.isNumber(t), let n = Double(t) { return .number(n) }
        return .string(text)
    }

    /// csvRows reads CSV as Python's csv.reader does by default: quotes
    /// start a field, "" in one is a quote, and a blank line is an empty row.
    private static func csvRows(_ text: String, delimiter: Unicode.Scalar) -> [[String]] {
        var rows: [[String]] = []
        var row: [String] = []
        var field = String.UnicodeScalarView()
        var quoted = false, wasQuoted = false
        let scalars = Array(text.unicodeScalars)
        var i = 0
        while i < scalars.count {
            let c = scalars[i]
            if quoted {
                if c == "\"" {
                    if i + 1 < scalars.count, scalars[i + 1] == "\"" {
                        field.append("\"")
                        i += 1
                    } else {
                        quoted = false
                    }
                } else {
                    field.append(c)
                }
                i += 1
                continue
            }
            if c == "\"", field.isEmpty, !wasQuoted {
                quoted = true
                wasQuoted = true
            } else if c == delimiter {
                row.append(String(field))
                field = String.UnicodeScalarView()
                wasQuoted = false
            } else if c == "\n" || c == "\r" {
                if c == "\r", i + 1 < scalars.count, scalars[i + 1] == "\n" { i += 1 }
                if !row.isEmpty || !field.isEmpty || wasQuoted { row.append(String(field)) }
                rows.append(row)
                row = []
                field = String.UnicodeScalarView()
                wasQuoted = false
            } else {
                field.append(c)
            }
            i += 1
        }
        if !row.isEmpty || !field.isEmpty || wasQuoted {
            row.append(String(field))
            rows.append(row)
        }
        return rows
    }
}

// MARK: - Numbers and text

/// A Metric's parts: its value as text, its unit, and its change, good or
/// bad as the Metric's better says.
struct SurfaceMetric: Equatable {
    enum Tone { case good, bad, flat }
    var text: String
    var unit: String
    var change: (text: String, dir: Int)?
    var tone: Tone
    var missing: Bool

    static func == (a: SurfaceMetric, b: SurfaceMetric) -> Bool {
        a.text == b.text && a.unit == b.unit && a.change?.text == b.change?.text && a.tone == b.tone && a.missing == b.missing
    }
}

enum SurfaceText {
    /// isNumber: text that reads as a number, as data.js's NUMBER.
    static func isNumber(_ text: some StringProtocol) -> Bool {
        var s = Substring(text)
        if s.hasPrefix("-") { s = s.dropFirst() }
        let mantissa = s.prefix { $0 != "e" && $0 != "E" }
        var exponent = s.dropFirst(mantissa.count)
        let parts = mantissa.split(separator: ".", maxSplits: 1, omittingEmptySubsequences: false)
        let whole = parts.first ?? "", fraction = parts.count > 1 ? parts[1] : ""
        func digits(_ t: Substring) -> Bool { t.allSatisfy { $0.isASCII && $0.isNumber } }
        guard digits(whole), digits(fraction), !whole.isEmpty || (parts.count > 1 && !fraction.isEmpty) else { return false }
        if !exponent.isEmpty {
            exponent = exponent.dropFirst()
            if exponent.hasPrefix("-") || exponent.hasPrefix("+") { exponent = exponent.dropFirst() }
            guard !exponent.isEmpty, digits(exponent) else { return false }
        }
        return true
    }

    static func asNumber(_ value: SurfaceValue) -> Double? {
        switch value {
        case let .number(n): return n
        case let .string(s):
            let t = s.trimmingCharacters(in: .whitespacesAndNewlines)
            return isNumber(t) ? Double(t) : nil
        default: return nil
        }
    }

    /// digits is a component's digits: a whole number from 0.
    static func digits(_ value: SurfaceValue?) -> Int? {
        guard let n = value?.number, n >= 0, n == n.rounded(), n <= 20 else { return nil }
        return Int(n)
    }

    /// num is a number as a surface shows it: digits fixed, or else as few
    /// as say it (8.5, 41.6, 0.12), and thousands apart (1 204).
    static func num(_ value: Double, digits: Int? = nil) -> String {
        if value.isNaN { return "nan" }
        if !value.isFinite { return value > 0 ? "inf" : "-inf" }
        var d = digits ?? 0
        if digits == nil {
            let a = abs(value)
            d = value == value.rounded() || a >= 100 ? 0 : a >= 10 ? 1 : 2
        }
        let fixed = String(format: "%.\(min(d, 20))f", abs(value))
        let parts = fixed.split(separator: ".", maxSplits: 1)
        var whole = String(parts.first ?? "0")
        var grouped = ""
        while whole.count > 3 {
            grouped = " " + whole.suffix(3) + grouped
            whole.removeLast(3)
        }
        grouped = whole + grouped
        var text = parts.count > 1 ? grouped + "." + parts[1] : grouped
        if digits == nil, text.contains(".") {
            while text.hasSuffix("0") { text.removeLast() }
            if text.hasSuffix(".") { text.removeLast() }
        }
        return (value < 0 && text.contains(where: { $0 != "0" && $0 != "." && $0 != " " }) ? "-" : value < 0 ? "-" : "") + text
    }

    /// shown is a value as text: "—" for none.
    static func shown(_ value: SurfaceValue?) -> String {
        guard let value else { return "—" }
        switch value {
        case .null: return "—"
        case let .bool(b): return b ? "true" : "false"
        case let .number(n): return num(n)
        case let .string(s): return s
        case .array, .object: return SurfaceJSON.encode(value)
        }
    }

    /// delta is the change from previous to value: ("▲8%", 1), or nil when
    /// there is none to tell.
    static func delta(_ value: SurfaceValue?, _ previous: SurfaceValue?) -> (text: String, dir: Int)? {
        guard let v = value.flatMap(asNumber), let p = previous.flatMap(asNumber), p != 0 else { return nil }
        let pct = (v - p) / abs(p) * 100
        if abs(pct) < 0.05 { return ("=0%", 0) }
        let size = abs(pct) < 1 ? String(format: "%.1f", abs(pct)) : String(format: "%.0f", abs(pct))
        return ("\(pct > 0 ? "▲" : "▼")\(size)%", pct > 0 ? 1 : -1)
    }

    /// previous is a Metric's previous value: its own, or the row before the
    /// last of the source its value is the last row of.
    static func previous(_ doc: SurfaceValue, _ c: SurfaceObject) -> SurfaceValue? {
        if let own = c["previous"] { return SurfacePointer.resolve(doc, own) }
        if let path = SurfacePointer.binding(c["value"]), path.hasPrefix("/sources/") {
            let rest = path.dropFirst("/sources/".count)
            if let slash = rest.firstIndex(of: "/"), rest[slash...].hasPrefix("/last/") {
                let source = rest[..<slash], field = rest[slash...].dropFirst("/last/".count)
                if !source.isEmpty, !field.isEmpty { return SurfacePointer.get(doc, "/sources/\(source)/rows/-2/\(field)") }
            }
        }
        return nil
    }

    static func metric(_ doc: SurfaceValue, _ c: SurfaceObject) -> SurfaceMetric {
        let value = SurfacePointer.resolve(doc, c["value"])
        let text: String
        if let n = value?.number { text = num(n, digits: digits(c["digits"])) } else { text = shown(value) }
        let missing = value == nil || value == .null
        let unit = !missing ? (c["unit"].map { $0.string ?? shown($0) } ?? "") : ""
        let change = delta(value, previous(doc, c))
        var tone = SurfaceMetric.Tone.flat
        if let change, change.dir != 0, let better = c.string("better"), better == "lower" || better == "higher" {
            tone = (change.dir < 0) == (better == "lower") ? .good : .bad
        }
        return SurfaceMetric(text: text, unit: unit, change: change, tone: tone, missing: missing)
    }

    static func metricText(_ doc: SurfaceValue, _ c: SurfaceObject) -> String {
        let m = metric(doc, c)
        return m.text + (m.unit.isEmpty ? "" : " \(m.unit)") + (m.change.map { " \($0.text)" } ?? "")
    }

    /// templated replaces the {…} of a template that ref fills.
    private static func templated(_ template: String, _ ref: (String) -> String?) -> String {
        var out = ""
        var rest = Substring(template)
        while let open = rest.firstIndex(of: "{") {
            out += rest[..<open]
            let after = rest[rest.index(after: open)...]
            if let close = after.firstIndex(where: { $0 == "}" || $0 == "{" }), after[close] == "}" {
                let inner = String(after[..<close])
                if let filled = valid(inner) ? ref(inner) : nil {
                    out += filled
                    rest = after[after.index(after: close)...]
                    continue
                }
            }
            out += "{"
            rest = after
        }
        return out + rest
    }

    private static func valid(_ ref: String) -> Bool {
        if ref.hasPrefix("/") { return true }
        guard ref.hasPrefix("#") else { return false }
        let id = ref.dropFirst()
        guard let first = id.first, first.isASCII, first.isLetter || first.isNumber else { return false }
        return id.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "_" || $0 == "." || $0 == "-") }
    }

    /// fill puts values in a template: {/pointer} a value of doc, {#id} a
    /// Metric as it shows.
    static func fill(_ doc: SurfaceValue, _ comps: [String: SurfaceObject], _ template: String) -> String {
        templated(template) { ref in
            if ref.hasPrefix("#") {
                guard let c = comps[String(ref.dropFirst())] else { return "?" }
                return metricText(doc, c)
            }
            return shown(SurfacePointer.get(doc, ref))
        }
    }

    /// fillItem fills a List's template with an item's fields.
    static func fillItem(_ item: SurfaceValue, _ template: String) -> String {
        templated(template) { ref in ref.hasPrefix("/") ? shown(SurfacePointer.get(item, ref)) : nil }
    }

    /// textOf is a text prop as it shows: a template filled, or a value bound.
    static func textOf(_ doc: SurfaceValue, _ comps: [String: SurfaceObject], _ value: SurfaceValue?) -> String {
        guard let value else { return "" }
        guard let v = SurfacePointer.resolve(doc, value) else { return "—" }
        if let s = v.string { return fill(doc, comps, s) }
        return shown(v)
    }
}

// MARK: - The call

/// SurfaceCall is what a UI call's result says of the version it made:
/// "UI bench v3 (.harness/ui/<session>/bench/v3.json)", then its warnings
/// and the surface as text.
struct SurfaceCall: Sendable, Equatable {
    let surface: String
    let version: Int
    let deleted: Bool
    let path: String
    var title = ""
    var warnings: [String] = []
    var text = ""
    /// The folder of the session the version is of: .harness/ui/<session>/….
    var session: String { path.split(separator: "/").dropFirst(2).first.map(String.init) ?? "" }

    /// isTool: the UI tool's name, or an alias of it, as "mcp__…__onion_UI".
    static func isTool(_ name: String?) -> Bool {
        guard let name else { return false }
        return name == "UI" || name.hasSuffix("_UI")
    }

    init?(tool: Tool?) {
        guard let tool, Self.isTool(tool.name), let output = tool.output else { return nil }
        self.init(output: output)
    }

    init?(output: String) {
        let first = output.prefix { $0 != "\n" }
        guard first.hasPrefix("UI ") else { return nil }
        var rest = first.dropFirst(3)
        let name = rest.prefix { $0.isASCII && ($0.isLowercase || $0.isNumber || $0 == "_" || $0 == "-") }
        guard let lead = name.first, lead.isLowercase || lead.isNumber else { return nil }
        rest = rest.dropFirst(name.count)
        guard rest.hasPrefix(" v") else { return nil }
        rest = rest.dropFirst(2)
        let number = rest.prefix { $0.isASCII && $0.isNumber }
        guard let version = Int(number) else { return nil }
        rest = rest.dropFirst(number.count)
        let deleted = rest.hasPrefix(" deleted")
        if deleted { rest = rest.dropFirst(" deleted".count) }
        guard rest.hasPrefix(" (.harness/ui/"), let end = rest.range(of: ".json)") else { return nil }
        let path = rest[rest.index(rest.startIndex, offsetBy: 2)..<end.lowerBound] + ".json"
        let inner = path.dropFirst(".harness/ui/".count).dropLast(".json".count)
        guard !inner.isEmpty, inner.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || "_./-".contains($0)) }) else { return nil }
        surface = String(name)
        self.version = version
        self.deleted = deleted
        self.path = String(path)
        let lines = output.split(separator: "\n", omittingEmptySubsequences: false)
        warnings = lines.filter { $0.hasPrefix("warning: ") }.map { String($0.dropFirst(9)) }
        if let head = lines.firstIndex(where: { $0.hasPrefix("◇ ") }) {
            let tag = " \(surface) v\(version)"
            let line = lines[head]
            title = String(line.hasSuffix(tag) ? line.dropFirst(2).dropLast(tag.count) : line.dropFirst(2))
            while title.last?.isWhitespace == true { title.removeLast() }
            text = lines[head...].joined(separator: "\n")
        }
    }

    /// eventText is what the agent reads of an event: "[UI deps v1] explain {…}".
    static func eventText(surface: String, version: Int, name: String, data: SurfaceValue?) -> String {
        var text = "[UI \(surface) v\(version)] \(name)"
        switch data {
        case nil, .null?: break
        case let .object(o)?: if !o.isEmpty { text += " " + SurfaceJSON.encode(.object(o)) }
        case let value?: text += " " + SurfaceJSON.encode(value)
        }
        return text
    }

    /// eventData is an event's data, its bindings read now: null for one
    /// that finds nothing.
    static func eventData(_ doc: SurfaceValue, _ data: SurfaceValue?) -> SurfaceObject {
        var out = SurfaceObject()
        for (key, raw) in data?.object?.pairs ?? [] {
            out[key] = SurfacePointer.resolve(doc, raw) ?? .null
        }
        return out
    }
}
