import CryptoKit
import Foundation
import Synchronization

/// What went wrong talking to the cockpit through its relay.
enum RelayError: LocalizedError, Sendable, Equatable {
    /// The relay does not take the token: it was set up anew, or the link
    /// is of another relay.
    case unauthorized
    /// The relay answers, but no cockpit is connected to it.
    case cockpitOffline
    /// Something answers at the address, and it is not a relay.
    case notARelay
    /// The relay's certificate is not the one paired.
    case pinMismatch
    case http(status: Int, message: String, code: String?)
    case network(String)
    case decoding(String)

    var errorDescription: String? {
        switch self {
        case .unauthorized: "The relay does not take this token. Pair again: /tunnel in the cockpit shows the code."
        case .cockpitOffline: "The relay is up, but the cockpit is not connected to it. Start the tunnel in the cockpit: /tunnel."
        case .notARelay: "That address answers, but not as a kou-conveyor relay."
        case .pinMismatch: "The relay's certificate is not the one this iPhone paired with. If the relay was set up anew, pair again."
        case let .http(status, message, _): "\(message) (\(status))"
        case let .network(message): message
        case let .decoding(message): "The cockpit answered something this app does not read: \(message)"
        }
    }

    /// Whether the relay or the cockpit is out of reach, rather than a
    /// request refused.
    var isConnectivity: Bool {
        switch self {
        case .cockpitOffline, .network, .pinMismatch, .unauthorized, .notARelay: true
        default: false
        }
    }
}

/// One event of a server-sent event stream.
struct ServerEvent: Sendable {
    let id: String?
    let data: String
}

/// RelayClient talks to the cockpit through its relay: every request has
/// the relay's token, and, for a relay with a certificate of its own, the
/// certificate is checked against the one paired.
final class RelayClient: Sendable {
    let config: RelayConfig
    private let trust: PinningDelegate
    private let session: URLSession
    /// Event streams last; the relay's heartbeats keep them from idling out.
    private let streaming: URLSession

    init(config: RelayConfig) {
        self.config = config
        trust = PinningDelegate(fingerprint: config.fingerprint)
        session = URLSession(configuration: Self.configuration(timeout: 30), delegate: trust, delegateQueue: nil)
        let long = Self.configuration(timeout: 50)
        long.timeoutIntervalForResource = 7 * 24 * 3600
        streaming = URLSession(configuration: long, delegate: trust, delegateQueue: nil)
    }

    deinit {
        session.invalidateAndCancel()
        streaming.invalidateAndCancel()
    }

    private static func configuration(timeout: TimeInterval) -> URLSessionConfiguration {
        let c = URLSessionConfiguration.ephemeral
        c.timeoutIntervalForRequest = timeout
        c.httpShouldSetCookies = false
        c.httpCookieAcceptPolicy = .never
        c.urlCache = nil
        c.requestCachePolicy = .reloadIgnoringLocalCacheData
        c.waitsForConnectivity = false
        return c
    }

    // MARK: Requests

    func url(_ path: String, query: [URLQueryItem] = []) -> URL {
        var components = URLComponents(url: config.url, resolvingAgainstBaseURL: false) ?? URLComponents()
        components.percentEncodedPath += path
        if !query.isEmpty { components.queryItems = query }
        return components.url ?? config.url
    }

    func request(_ method: String, _ path: String, query: [URLQueryItem] = []) -> URLRequest {
        var request = URLRequest(url: url(path, query: query))
        request.httpMethod = method
        request.setValue("Bearer \(config.token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        return request
    }

    func get<T: Decodable & Sendable>(_ path: String, query: [URLQueryItem] = [], as type: T.Type = T.self) async throws -> T {
        try Self.decode(type, from: try await perform(request("GET", path, query: query)))
    }

    func send<T: Decodable & Sendable>(_ method: String, _ path: String, body: some Encodable & Sendable, as type: T.Type = T.self) async throws -> T {
        try Self.decode(type, from: try await send(method, path, body: body))
    }

    @discardableResult
    func send(_ method: String, _ path: String, body: some Encodable & Sendable) async throws -> Data {
        var request = request(method, path)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSON.encoder().encode(body)
        return try await perform(request)
    }

    func perform(_ request: URLRequest) async throws -> Data {
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            throw failure(error)
        }
        guard let http = response as? HTTPURLResponse else { throw RelayError.network("The relay did not answer over HTTP.") }
        guard (200..<300).contains(http.statusCode) else { throw Self.failure(status: http.statusCode, body: data) }
        return data
    }

    /// The relay's own word on itself and on the cockpit connected to it.
    func health() async throws -> RelayHealth {
        let health: RelayHealth
        do {
            health = try await get("/_relay/health")
        } catch RelayError.http(404, _, _), RelayError.decoding {
            throw RelayError.notARelay
        }
        guard health.relay == "kou-conveyor-relay" else { throw RelayError.notARelay }
        guard health.authorized == true else { throw RelayError.unauthorized }
        return health
    }

    // MARK: Event streams

    /// Streams the events at path. The stream ends when the server ends it
    /// (204 says there is nothing more to send), and fails with what broke
    /// it; lastEventID resumes after the event of that ID.
    func events(_ path: String, lastEventID: String? = nil) -> AsyncThrowingStream<ServerEvent, any Error> {
        var request = request("GET", path)
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        if let lastEventID { request.setValue(lastEventID, forHTTPHeaderField: "Last-Event-ID") }
        let ready = request
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let (bytes, response) = try await self.streaming.bytes(for: ready, delegate: self.trust)
                    guard let http = response as? HTTPURLResponse else { throw RelayError.network("The relay did not answer over HTTP.") }
                    if http.statusCode == 204 {
                        continuation.finish()
                        return
                    }
                    guard (200..<300).contains(http.statusCode) else {
                        var body = Data()
                        for try await byte in bytes {
                            body.append(byte)
                            if body.count > 64 << 10 { break }
                        }
                        throw Self.failure(status: http.statusCode, body: body)
                    }
                    var parser = EventParser()
                    var line: [UInt8] = []
                    line.reserveCapacity(4096)
                    for try await byte in bytes {
                        guard byte == UInt8(ascii: "\n") else {
                            line.append(byte)
                            continue
                        }
                        if line.last == UInt8(ascii: "\r") { line.removeLast() }
                        if let event = parser.feed(String(decoding: line, as: UTF8.self)) {
                            continuation.yield(event)
                        }
                        line.removeAll(keepingCapacity: true)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: self.failure(error))
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    // MARK: Errors

    private func failure(_ error: any Error) -> any Error {
        if error is RelayError || error is CancellationError { return error }
        guard let url = error as? URLError else { return error }
        if trust.refused() { return RelayError.pinMismatch }
        switch url.code {
        case .cancelled: return CancellationError()
        case .serverCertificateUntrusted, .serverCertificateHasBadDate, .serverCertificateNotYetValid, .serverCertificateHasUnknownRoot:
            if config.fingerprint != nil {
                return RelayError.network("The relay's certificate could not be checked against the one paired.")
            }
            return RelayError.network("The relay's certificate is not trusted, and this iPhone has no fingerprint to pin it by. Pair with the code /tunnel shows.")
        case .notConnectedToInternet: return RelayError.network("This iPhone is offline.")
        case .cannotFindHost, .dnsLookupFailed: return RelayError.network("No host \(config.address) is found.")
        case .cannotConnectToHost: return RelayError.network("Nothing answers at \(config.address): is the relay running?")
        case .timedOut: return RelayError.network("The relay at \(config.address) did not answer in time.")
        case .networkConnectionLost: return RelayError.network("The connection to the relay was lost.")
        default: return RelayError.network(url.localizedDescription)
        }
    }

    static func failure(status: Int, body: Data) -> RelayError {
        struct Body: Decodable {
            let error: String?
            let code: String?
        }
        let parsed = try? JSONDecoder().decode(Body.self, from: body)
        if parsed?.code == "agent_offline" { return .cockpitOffline }
        if status == 401 { return .unauthorized }
        let message = parsed?.error ?? HTTPURLResponse.localizedString(forStatusCode: status).capitalized
        return .http(status: status, message: message, code: parsed?.code)
    }

    static func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        do {
            return try JSON.decoder().decode(type, from: data)
        } catch let error as DecodingError {
            throw RelayError.decoding(Self.describe(error))
        }
    }

    private static func describe(_ error: DecodingError) -> String {
        func path(_ context: DecodingError.Context) -> String {
            context.codingPath.map { $0.intValue.map { "[\($0)]" } ?? $0.stringValue }.joined(separator: ".")
        }
        switch error {
        case let .keyNotFound(key, context): return "no \(key.stringValue) at \(path(context))"
        case let .typeMismatch(_, context), let .valueNotFound(_, context), let .dataCorrupted(context):
            return "\(context.debugDescription) at \(path(context))"
        @unknown default: return error.localizedDescription
        }
    }
}

/// EventParser reads the lines of an event stream into its events.
struct EventParser {
    private var id: String?
    private var data: [String] = []

    mutating func feed(_ line: String) -> ServerEvent? {
        if line.isEmpty {
            defer { data.removeAll() }
            guard !data.isEmpty else { return nil }
            return ServerEvent(id: id, data: data.joined(separator: "\n"))
        }
        if line.hasPrefix(":") { return nil }
        let field: Substring
        var value: Substring
        if let colon = line.firstIndex(of: ":") {
            field = line[..<colon]
            value = line[line.index(after: colon)...]
            if value.hasPrefix(" ") { value = value.dropFirst() }
        } else {
            field = Substring(line)
            value = ""
        }
        switch field {
        case "data": data.append(String(value))
        case "id": id = String(value)
        default: break
        }
        return nil
    }
}

/// PinningDelegate checks a relay's certificate against the fingerprint
/// paired: the SHA-256 of the certificate, as `kou-conveyor-relay` prints
/// it. Without a fingerprint, the system's CAs decide.
///
/// It answers the challenge both for the session and for a task: a stream
/// (`bytes(for:)`) asks its task's delegate only, and without an answer
/// there the system's CAs refuse the relay's own certificate.
final class PinningDelegate: NSObject, URLSessionTaskDelegate, Sendable {
    let fingerprint: String?
    private let mismatch = Mutex(false)

    init(fingerprint: String?) {
        self.fingerprint = fingerprint
    }

    /// Whether a certificate was refused since this was last asked.
    func refused() -> Bool {
        mismatch.withLock { refused in
            defer { refused = false }
            return refused
        }
    }

    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping @Sendable (URLSession.AuthChallengeDisposition, URLCredential?) -> Void)
    {
        let (disposition, credential) = answer(challenge)
        completionHandler(disposition, credential)
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping @Sendable (URLSession.AuthChallengeDisposition, URLCredential?) -> Void)
    {
        let (disposition, credential) = answer(challenge)
        completionHandler(disposition, credential)
    }

    private func answer(_ challenge: URLAuthenticationChallenge) -> (URLSession.AuthChallengeDisposition, URLCredential?) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust, let fingerprint
        else {
            return (.performDefaultHandling, nil)
        }
        guard let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate], let leaf = chain.first else {
            mismatch.withLock { $0 = true }
            return (.cancelAuthenticationChallenge, nil)
        }
        let der = SecCertificateCopyData(leaf) as Data
        let found = SHA256.hash(data: der).map { String(format: "%02x", $0) }.joined()
        guard found == fingerprint else {
            mismatch.withLock { $0 = true }
            return (.cancelAuthenticationChallenge, nil)
        }
        return (.useCredential, URLCredential(trust: trust))
    }
}

/// Path segments, escaped.
enum API {
    private static let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~")

    static func path(_ segments: String...) -> String {
        "/" + segments.map { $0.addingPercentEncoding(withAllowedCharacters: allowed) ?? $0 }.joined(separator: "/")
    }

    /// The path of a workspace's resource: /api/w/{ws}/….
    static func workspace(_ ws: String, _ rest: String...) -> String {
        "/api/w/" + (ws.addingPercentEncoding(withAllowedCharacters: allowed) ?? ws)
            + rest.map { "/" + ($0.addingPercentEncoding(withAllowedCharacters: allowed) ?? $0) }.joined()
    }
}

/// The cockpit's JSON: Go writes its times in RFC 3339 with nanoseconds.
enum JSON {
    static func decoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let container = try decoder.singleValueContainer()
            let text = try container.decode(String.self)
            guard let date = RFC3339.date(text) else {
                throw DecodingError.dataCorruptedError(in: container, debugDescription: "not an RFC 3339 time: \(text)")
            }
            return date
        }
        return decoder
    }

    static func encoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        return encoder
    }
}

enum RFC3339 {
    // ISO8601DateFormatter is safe to use from any thread.
    nonisolated(unsafe) private static let fractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    nonisolated(unsafe) private static let whole: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()

    static func date(_ text: String) -> Date? {
        guard let dot = text.firstIndex(of: ".") else { return whole.date(from: text) }
        // The formatter reads milliseconds; Go writes up to nanoseconds.
        let rest = text[text.index(after: dot)...]
        let end = rest.firstIndex(where: { !$0.isNumber }) ?? rest.endIndex
        let digits = rest[..<end].prefix(3)
        return fractional.date(from: String(text[..<dot]) + "." + digits + String(rest[end...]))
    }
}
