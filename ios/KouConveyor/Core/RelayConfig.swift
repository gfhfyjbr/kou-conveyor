import Foundation
import Security

/// Where the relay is and how to reach it: what the pairing code /tunnel
/// shows in the cockpit says (kou-conveyor-web tunnel link).
struct RelayConfig: Codable, Sendable, Hashable {
    /// The relay's base URL, http(s)://host:port, without a trailing slash.
    var url: URL
    /// The relay's secret, sent with every request.
    var token: String
    /// The SHA-256 of the relay's certificate, in hex, when the relay has a
    /// certificate of its own that no CA vouches for; nil trusts the system's
    /// CAs.
    var fingerprint: String?

    /// host:port, as the user knows the relay.
    var address: String {
        guard let host = url.host() else { return url.absoluteString }
        if let port = url.port { return "\(host):\(port)" }
        return host
    }

    var isEncrypted: Bool { url.scheme == "https" }

    /// The fingerprint as the cockpit's dialog shows it: its first bytes.
    var shortFingerprint: String? {
        guard let fingerprint else { return nil }
        let pairs = stride(from: 0, to: min(fingerprint.count, 16), by: 2).map { i -> String in
            let start = fingerprint.index(fingerprint.startIndex, offsetBy: i)
            return String(fingerprint[start..<fingerprint.index(start, offsetBy: 2)])
        }
        return pairs.joined(separator: ":") + "…"
    }
}

enum PairingError: LocalizedError, Equatable {
    case notALink
    case missingURL
    case badURL
    case missingToken
    case badFingerprint

    var errorDescription: String? {
        switch self {
        case .notALink: "That is not a pairing link. In the cockpit, /tunnel shows one: kouconveyor://pair?…"
        case .missingURL: "The link does not say where the relay is."
        case .badURL: "The relay's address must be an http or https URL."
        case .missingToken: "The link has no token."
        case .badFingerprint: "The certificate fingerprint must be a SHA-256 in hex (64 digits)."
        }
    }
}

extension RelayConfig {
    static let scheme = "kouconveyor"

    /// Reads a pairing link: kouconveyor://pair?url=…&token=…&fp=…, or the
    /// browser link the cockpit shows, <relay>/?kou_token=….
    init(link: String) throws {
        let text = link.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let components = URLComponents(string: text), let scheme = components.scheme?.lowercased() else {
            throw PairingError.notALink
        }
        let query = Dictionary((components.queryItems ?? []).map { ($0.name, $0.value ?? "") }, uniquingKeysWith: { _, last in last })
        switch scheme {
        case Self.scheme:
            guard components.host == "pair" else { throw PairingError.notALink }
            guard let raw = query["url"], !raw.isEmpty else { throw PairingError.missingURL }
            try self.init(url: raw, token: query["token"] ?? "", fingerprint: query["fp"])
        case "http", "https":
            guard let token = query["kou_token"] else { throw PairingError.notALink }
            var base = components
            base.path = ""
            base.query = nil
            base.fragment = nil
            guard let raw = base.string else { throw PairingError.badURL }
            try self.init(url: raw, token: token, fingerprint: nil)
        default:
            throw PairingError.notALink
        }
    }

    /// A relay given by hand.
    init(url raw: String, token: String, fingerprint: String?) throws {
        var text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        while text.hasSuffix("/") { text.removeLast() }
        if !text.contains("://") { text = "https://" + text }
        guard let url = URL(string: text), let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
              let host = url.host(), !host.isEmpty, url.query() == nil
        else { throw PairingError.badURL }
        let token = token.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty, !token.contains(where: \.isWhitespace) else { throw PairingError.missingToken }
        var pin: String?
        if let fingerprint, !fingerprint.trimmingCharacters(in: .whitespaces).isEmpty {
            pin = try Self.normalize(fingerprint: fingerprint)
        }
        self.init(url: url, token: token, fingerprint: pin)
    }

    /// The fingerprint as the relay prints it: 64 lowercase hex digits,
    /// whatever colons and spaces it was given with.
    static func normalize(fingerprint: String) throws -> String {
        let hex = fingerprint.lowercased().filter { !":- \n\t".contains($0) }
        guard hex.count == 64, hex.allSatisfy(\.isHexDigit) else { throw PairingError.badFingerprint }
        return hex
    }
}

/// The paired relay, kept in the keychain: its token is a key to the
/// cockpit, and so to the computer it runs on.
enum RelayStore {
    private static let service = "io.github.gfhfyjbr.kouconveyor.relay"
    private static let account = "current"

    static func load() -> RelayConfig? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess, let data = item as? Data else {
            return nil
        }
        return try? JSONDecoder().decode(RelayConfig.self, from: data)
    }

    static func save(_ config: RelayConfig) throws {
        let data = try JSONEncoder().encode(config)
        clear()
        let item: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData as String: data,
        ]
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(status), userInfo: [
                NSLocalizedDescriptionKey: "The keychain did not keep the relay (\(status)).",
            ])
        }
    }

    static func clear() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(query as CFDictionary)
    }
}
