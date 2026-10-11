import AVFoundation
import Foundation
import Observation
import Synchronization
import UIKit

/// VoiceInput takes dictation for the composer. What the microphone hears
/// streams to the cockpit as 16 kHz mono 16-bit PCM, over a WebSocket through
/// the relay (GET /api/w/{ws}/voice/socket, cmd/kou-conveyor-web/
/// voicesocket.go), and the cockpit streams it on to the speech-to-text
/// provider chosen in dictation's settings, whose key never leaves the Mac.
/// The words come back as they are said, the last few revised until the
/// provider is sure of them, or, from the providers that take the whole
/// take, once it stops; stopping asks the provider to be sure of all of
/// them. The cockpit tells it the languages of the iPhone, and the names of
/// the workspace's folders and files, which it then spells as the workspace
/// does.
@MainActor
@Observable
final class VoiceInput {
    enum State: Equatable {
        case idle
        /// Waiting for the microphone.
        case starting
        case listening
        /// Stopped: waiting for the provider to be sure of the last words.
        case finishing
    }

    private(set) var state: State = .idle
    /// What was said so far: the words the provider is sure of, then its
    /// guess at the last ones.
    private(set) var transcript = ""
    /// What went wrong with the last take.
    private(set) var failure: String?

    /// Counts the takes: what is left running of one finds it is over.
    @ObservationIgnored private var take = 0
    @ObservationIgnored private var engine: AVAudioEngine?
    @ObservationIgnored private var pump: VoicePump?
    @ObservationIgnored private var socket: URLSessionWebSocketTask?
    /// The socket is open.
    @ObservationIgnored private var open = false
    /// The provider is sure of all the audio sent.
    @ObservationIgnored private var done = false
    /// The provider that listens, once the cockpit says.
    @ObservationIgnored private var provider = ""
    @ObservationIgnored private var observers: [any NSObjectProtocol] = []

    /// start starts a take: the microphone, and the stream it goes to.
    func start(client: RelayClient, workspace: String) {
        guard state == .idle else { return }
        take &+= 1
        state = .starting
        transcript = ""
        failure = nil
        provider = ""
        let current = take
        Task { await begin(current, client: client, workspace: workspace) }
    }

    /// stop ends the take once the provider is sure of the last words.
    func stop() {
        guard state == .starting || state == .listening else { return }
        state = .finishing
        let current = take
        Task { await finish(current) }
    }

    /// cancel ends the take at once.
    func cancel() {
        guard state != .idle else { return }
        end()
    }

    // MARK: A take

    private func begin(_ current: Int, client: RelayClient, workspace: String) async {
        guard await AVAudioApplication.requestRecordPermission() else {
            return fail(current, "Dictation needs the microphone: allow it for kou-conveyor in Settings.")
        }
        guard take == current else { return }
        // The stream opens while the microphone does: what is said meanwhile
        // waits for it.
        connect(current, client: client, workspace: workspace)
        do {
            try listen()
        } catch {
            return fail(current, "The microphone could not be opened: \(error.localizedDescription)")
        }
        if take == current, state == .starting { state = .listening }
    }

    /// listen opens the microphone: the pump turns what it hears into the
    /// stream's audio.
    private func listen() throws {
        let audio = AVAudioSession.sharedInstance()
        try audio.setCategory(.record, mode: .measurement)
        try audio.setActive(true, options: .notifyOthersOnDeactivation)
        let engine = AVAudioEngine()
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else { throw VoiceError("No microphone was found.") }
        let pump = try VoicePump(from: format)
        if let socket, open { pump.open(socket) }
        input.installTap(onBus: 0, bufferSize: 4096, format: format, block: pump.tap())
        engine.prepare()
        do {
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            throw error
        }
        self.engine = engine
        self.pump = pump
        observe(engine)
    }

    /// connect opens the stream; once it is open, the audio heard meanwhile
    /// goes, then the rest as it comes.
    private func connect(_ current: Int, client: RelayClient, workspace: String) {
        let socket = client.webSocket(API.workspace(workspace, "voice", "socket"),
                                      query: [URLQueryItem(name: "languages", value: Locale.preferredLanguages.joined(separator: ","))])
        socket.maximumMessageSize = 1 << 20
        self.socket = socket
        socket.resume()
        receive(current, socket)
        // The pong says that the socket is open.
        socket.sendPing { [weak self] error in
            Task { @MainActor in self?.opened(current, socket, error) }
        }
        // A socket that neither opens nor fails, as behind a proxy that
        // holds it, would leave the take listening to no one.
        Task {
            try? await Task.sleep(for: .seconds(10))
            if take == current, !open { fail(current, "The cockpit could not be reached: dictation's stream did not open.") }
        }
    }

    private func opened(_ current: Int, _ socket: URLSessionWebSocketTask, _ error: (any Error)?) {
        guard take == current, error == nil else { return }
        open = true
        pump?.open(socket)
    }

    /// receive reads what the cockpit says until the take is over.
    private func receive(_ current: Int, _ socket: URLSessionWebSocketTask) {
        Task {
            while take == current {
                let message: URLSessionWebSocketTask.Message
                do {
                    message = try await socket.receive()
                } catch {
                    return closed(current, socket)
                }
                if case let .string(text) = message, take == current, !answer(current, text) { return }
            }
        }
    }

    /// answer takes what the cockpit says: the provider listens; all the
    /// words it is sure of and its guess at those after them, sent anew each
    /// time; that it is sure of them all; or what went wrong. False once the
    /// take is over.
    private func answer(_ current: Int, _ text: String) -> Bool {
        guard let object = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] else { return true }
        switch object["type"] as? String {
        case "ready":
            provider = object["name"] as? String ?? ""
        case "text":
            let words = (object["final"] as? String ?? "") + (object["partial"] as? String ?? "")
            let said = String(words.drop(while: \.isWhitespace))
            if said != transcript { transcript = said }
        case "done":
            done = true
            return false
        case "error":
            var message = object["message"] as? String ?? "Dictation failed."
            // The dialog it speaks of is the browser cockpit's.
            if object["setup"] as? Bool == true { message += " The dialog is the cockpit's, in a browser: /voice." }
            fail(current, message)
            return false
        default:
            break
        }
        return true
    }

    private func closed(_ current: Int, _ socket: URLSessionWebSocketTask) {
        guard take == current, !done else { return }
        fail(current, open ? "Dictation's stream closed (\(socket.closeCode.rawValue))." : "The cockpit could not be reached.")
    }

    /// finish hears the end of the last word out, lets the microphone go,
    /// and asks the provider to be sure of what it heard.
    private func finish(_ current: Int) async {
        try? await Task.sleep(for: .milliseconds(200))
        guard take == current else { return }
        silence()
        // What was heard went nowhere: the take fails rather than end as if
        // all was said.
        guard await until(current, within: .seconds(10), { self.open }), let socket else {
            return fail(current, "The cockpit could not be reached: dictation's stream did not open.")
        }
        // The audio sent before goes first: the socket keeps the order.
        socket.send(.string(#"{"type":"finish"}"#)) { _ in }
        // The cockpit gives up before this: those that take the whole take
        // may need a while for a long one.
        guard await until(current, within: .seconds(75), { self.done }) else {
            let name = provider.isEmpty ? "The provider" : provider
            return fail(current, "\(name) did not give the last words in time.")
        }
        if take == current { end() }
    }

    /// until waits for a condition, a while at most: false if it did not
    /// come to be, or the take ended.
    private func until(_ current: Int, within limit: Duration, _ condition: () -> Bool) async -> Bool {
        let deadline = ContinuousClock.now.advanced(by: limit)
        while take == current {
            if condition() { return true }
            if ContinuousClock.now >= deadline { return false }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return false
    }

    /// silence lets the microphone go: what it heard is all the stream gets.
    private func silence() {
        pump?.seal()
        guard let engine else { return }
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        self.engine = nil
    }

    /// end closes the stream and lets the microphone go.
    private func end() {
        take &+= 1
        silence()
        pump?.close()
        pump = nil
        socket?.cancel(with: .normalClosure, reason: nil)
        socket = nil
        open = false
        done = false
        state = .idle
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
        observers = []
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    private func fail(_ current: Int, _ message: String) {
        guard take == current else { return }
        failure = message
        end()
    }

    /// observe stops the take when the microphone is taken away: a call, the
    /// app going to the background, headphones put on or taken off.
    private func observe(_ engine: AVAudioEngine) {
        let center = NotificationCenter.default
        let halt: @Sendable (Notification) -> Void = { [weak self] _ in
            Task { @MainActor in self?.stop() }
        }
        observers = [
            center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main, using: halt),
            center.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main, using: halt),
            center.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main, using: halt),
        ]
    }
}

struct VoiceError: LocalizedError {
    let errorDescription: String?

    init(_ message: String) {
        errorDescription = message
    }
}

/// VoicePump turns what the microphone hears into the stream's audio: 16 kHz
/// mono 16-bit PCM, sent as it comes once the stream is open, and kept until
/// then. The audio engine calls it on a thread of its own: it is no actor's.
final class VoicePump: @unchecked Sendable {
    private struct Pending {
        var socket: URLSessionWebSocketTask?
        var queue: [Data] = []
        /// No more audio is taken.
        var sealed = false
    }

    private let converter: AVAudioConverter
    private let format: AVAudioFormat
    private let pending = Mutex(Pending())

    init(from input: AVAudioFormat) throws {
        guard let format = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16000, channels: 1, interleaved: true),
              let converter = AVAudioConverter(from: input, to: format)
        else { throw VoiceError("The microphone's audio cannot be converted for the stream.") }
        converter.downmix = true
        self.format = format
        self.converter = converter
    }

    /// tap is the block the microphone's node calls with what it heard.
    func tap() -> AVAudioNodeTapBlock {
        { [self] buffer, _ in convert(buffer) }
    }

    private func convert(_ buffer: AVAudioPCMBuffer) {
        let ratio = format.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount((Double(buffer.frameLength) * ratio).rounded(.up)) + 32
        guard let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else { return }
        // The converter calls the block before convert returns, on this
        // thread: nothing runs concurrently with it.
        nonisolated(unsafe) var given = false
        nonisolated(unsafe) let source = buffer
        var error: NSError?
        let status = converter.convert(to: output, error: &error) { _, inputStatus in
            // The converter keeps its state between buffers: "no data now"
            // rather than "end of stream".
            if given {
                inputStatus.pointee = .noDataNow
                return nil
            }
            given = true
            inputStatus.pointee = .haveData
            return source
        }
        guard status != .error, output.frameLength > 0, let samples = output.int16ChannelData else { return }
        let data = Data(bytes: samples[0], count: Int(output.frameLength) * MemoryLayout<Int16>.size)
        pending.withLock { pending in
            guard !pending.sealed else { return }
            if let socket = pending.socket {
                socket.send(.data(data)) { _ in }
            } else {
                pending.queue.append(data)
            }
        }
    }

    /// open sends what was heard before the stream opened, and the rest as
    /// it comes.
    func open(_ socket: URLSessionWebSocketTask) {
        pending.withLock { pending in
            for data in pending.queue { socket.send(.data(data)) { _ in } }
            pending.queue = []
            pending.socket = socket
        }
    }

    /// seal takes no more audio: what was heard is all there is.
    func seal() {
        pending.withLock { $0.sealed = true }
    }

    /// close drops what was not sent.
    func close() {
        pending.withLock { $0 = Pending(sealed: true) }
    }
}
