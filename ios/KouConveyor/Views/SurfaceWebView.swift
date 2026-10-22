import CryptoKit
import SwiftUI
import UIKit
import WebKit

/// SurfaceHtml is a surface's Html component: a page of the agent's own, in
/// a web view of its own.
///
/// The cockpit makes the page, as it does for its own frame: the agent's
/// html, or its file as it is now, under the bridge that gives it
/// window.kou. Here the page has no origin of the cockpit's and reaches
/// nothing: its content security policy lets in only what it has inline, a
/// rule list blocks every load off the device, and what it keeps goes with
/// the web view. What it asks for — to set an input, to send the agent an
/// event, to open a file — is weighed here, as the web cockpit weighs it.
struct SurfaceHtml: View {
    let env: SurfaceEnv
    let c: SurfaceObject
    @Environment(\.colorScheme) private var scheme
    /// The page, ready for a web view.
    @State private var page: String?
    /// Why there is no page.
    @State private var problem: String?
    /// Whether the cockpit is too old to make the page.
    @State private var older = false
    /// How tall the page is, by what it says.
    @State private var height: CGFloat = 120
    /// What became of what the page asked for, when it did not go.
    @State private var note: String?
    /// When the page last sent the agent an event.
    @State private var sent: ContinuousClock.Instant?
    /// Whether the page's bridge said it is ready.
    @State private var bridged = false

    private static let olderBridge = "This cockpit is older than the app's Html: the page gets no values or theme "
        + "until the cockpit is rebuilt."

    private var id: String { c.string("id") ?? "" }
    private var alt: String { c.string("alt") ?? "" }
    private var inputs: [String] { (c.array("inputs") ?? []).compactMap(\.string) }
    private var actions: [String] { (c.array("actions") ?? []).compactMap(\.string) }
    /// The height the call gave the page, if it gave one.
    private var fixed: CGFloat? { c.number("height").map { CGFloat(min(max($0, 24), 1600)) } }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if older {
                SurfaceUnknown(kind: "HTML", alt: alt,
                               note: "This cockpit is older than live HTML: it frames the page once it is rebuilt.")
            } else if let page {
                SurfaceWebView(html: page, data: data, theme: SurfaceTheme.json(dark: scheme == .dark)) { message, active in
                    handle(message, active: active)
                }
                .frame(maxWidth: .infinity)
                .frame(height: fixed ?? height)
                .clipShape(.rect(cornerRadius: .corner(10)))
            } else if let problem {
                SurfaceUnknown(kind: "HTML", alt: alt, note: problem)
            } else {
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text("Framing the page…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, minHeight: 48, alignment: .leading)
            }
            if let note {
                Label(note, systemImage: "exclamationmark.circle")
                    .font(.caption2)
                    .foregroundStyle(.orange)
            }
        }
        .task(id: "\(env.path)#\(id)#\(env.live)") { await frame() }
        .task(id: page) { await awaitBridge() }
    }

    /// awaitBridge says so when the page's bridge does not answer: the
    /// cockpit made the page before it could talk to the app.
    private func awaitBridge() async {
        guard page != nil else { return }
        try? await Task.sleep(for: .seconds(5))
        if !Task.isCancelled, page != nil, !bridged { note = Self.olderBridge }
    }

    /// data is the values of the pointers the page reads, in JSON.
    private var data: String {
        var values = SurfaceObject()
        for pointer in inputs {
            values[pointer] = SurfacePointer.get(env.doc, pointer) ?? .null
        }
        return SurfaceJSON.encode(.object(values))
    }

    /// frame fetches the page and, while the surface is live and the page is
    /// a file's, fetches it again every few seconds to follow the file.
    private func frame() async {
        guard !id.isEmpty else {
            problem = "This Html has no id."
            return
        }
        await SurfaceWebRules.prepare()
        let follows = env.live && c.string("file") != nil
        var last: Data?
        while !Task.isCancelled {
            do {
                if let got = try await fetch(unless: last), got != last {
                    last = got
                    bridged = false
                    page = SurfaceWebPage.prepare(String(decoding: got, as: UTF8.self))
                    problem = nil
                }
            } catch {
                if Task.isCancelled { return }
                if let relay = error as? RelayError, case let .http(status, message, _) = relay,
                   status == 404, message == "not found" {
                    // The cockpit has no such route: it is older than Html.
                    older = true
                    return
                }
                if page == nil { problem = SurfaceBook.say(error) }
            }
            guard follows else { return }
            await AppFront.poll(.seconds(3))
        }
    }

    /// fetch gets the page; nil while it is the one got last. The cockpit
    /// tags a page by its hash, as for the web cockpit's frame.
    private func fetch(unless last: Data?) async throws -> Data? {
        let path = API.workspace(env.workspace, "sessions", env.call.session, "surfaces", env.call.surface, "html", id)
        var request = env.client.request("GET", path, query: [URLQueryItem(name: "v", value: String(env.call.version))])
        if let last {
            let tag = SHA256.hash(data: last).prefix(12)
                .map { $0 < 16 ? "0" + String($0, radix: 16) : String($0, radix: 16) }.joined()
            request.setValue("\"\(tag)\"", forHTTPHeaderField: "If-None-Match")
        }
        do {
            return try await env.client.perform(request)
        } catch RelayError.http(status: 304, message: _, code: _) {
            return nil
        }
    }

    /// handle takes what the page asks for; active is whether a click or a
    /// key of the user's is behind it.
    private func handle(_ message: SurfaceObject, active: Bool) {
        switch message.string("type") {
        case "ready":
            bridged = true
            // A cockpit before bridge 2 hears the app by another way.
            if (message.number("bridge") ?? 1) < 2 { note = Self.olderBridge }
        case "size":
            if let h = message.number("height"), h.isFinite {
                height = min(max(CGFloat(h.rounded(.up)), 24), 1600)
            }
        case "action":
            act(message, active: active)
        case "set":
            set(message)
        case "open":
            open(message, active: active)
        case "error":
            note = "The page's script failed: " + String((message.string("message") ?? "").prefix(300))
        default:
            break
        }
    }

    private func act(_ message: SurfaceObject, active: Bool) {
        let name = message.string("name") ?? ""
        guard env.live else {
            note = "This is a snapshot: events work in the newest version."
            return
        }
        guard actions.contains(name) else {
            note = "\"\(name)\" is not one of this Html's actions, so it was not sent."
            return
        }
        guard active else {
            note = "An event goes to the agent only from a click or a key in the page."
            return
        }
        let now = ContinuousClock.now
        if let sent, now - sent < .seconds(1) { return }
        let data = message["data"].flatMap { $0.isNull ? nil : $0 }
        if let data, SurfaceJSON.encode(data).utf8.count > 8 << 10 {
            note = "The event's data is over 8 KB, so it was not sent."
            return
        }
        sent = now
        note = nil
        let text = SurfaceCall.eventText(surface: env.call.surface, version: env.call.version, name: name, data: data)
        let book = env.book
        Task {
            if !(await book.send(text)) { note = "The event did not reach the agent." }
        }
    }

    private func set(_ message: SurfaceObject) {
        let pointer = message.string("pointer") ?? ""
        guard env.live else {
            note = "This is a snapshot: events work in the newest version."
            return
        }
        guard inputs.contains(pointer) else {
            note = "\"\(pointer)\" is not one of this Html's inputs, so it was not set."
            return
        }
        guard let first = SurfacePointer.tokens(pointer)?.first, first != "sources", first != "sourceInfo" else {
            note = "The page cannot set \(pointer): the files' values are not its to set."
            return
        }
        let value = message["value"] ?? .null
        guard SurfaceJSON.encode(value).utf8.count <= 32 << 10 else {
            note = "The value for \(pointer) is over 32 KB, so it was not set."
            return
        }
        env.book.set(env.path, pointer, value)
    }

    private func open(_ message: SurfaceObject, active: Bool) {
        let path = message.string("path") ?? ""
        guard active else {
            note = "A file opens only from a click or a key in the page."
            return
        }
        let files = Set(env.version.sources.map(\.file) + [c.string("file")].compactMap { $0 })
        guard files.contains(path) else {
            note = "\(path) is not one of this surface's files, so it was not opened."
            return
        }
        env.open(path, message.int("line"))
    }
}

/// SurfaceWebView is the web view an Html's page is drawn in.
struct SurfaceWebView: UIViewRepresentable {
    let html: String
    /// The values of the pointers the page reads, in JSON.
    let data: String
    /// The theme, in JSON.
    let theme: String
    /// What the page says, and whether a click or a key of the user's is
    /// behind it.
    let onMessage: (SurfaceObject, Bool) -> Void

    func makeCoordinator() -> SurfaceWebCoordinator { SurfaceWebCoordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.preferences.javaScriptCanOpenWindowsAutomatically = false
        config.dataDetectorTypes = []
        config.allowsInlineMediaPlayback = true
        config.mediaTypesRequiringUserActionForPlayback = .all
        let contents = config.userContentController
        let world = SurfaceWebCoordinator.world
        contents.addUserScript(WKUserScript(source: SurfaceWebCoordinator.listener, injectionTime: .atDocumentStart,
                                            forMainFrameOnly: true, in: world))
        contents.add(SurfaceWebHandler(context.coordinator), contentWorld: world, name: "kou")
        if let rules = SurfaceWebRules.list { contents.add(rules) }
        let view = WKWebView(frame: .zero, configuration: config)
        view.isOpaque = false
        view.backgroundColor = .clear
        view.scrollView.backgroundColor = .clear
        view.scrollView.isScrollEnabled = false
        view.scrollView.bounces = false
        view.scrollView.minimumZoomScale = 1
        view.scrollView.maximumZoomScale = 1
        view.scrollView.contentInsetAdjustmentBehavior = .never
        view.allowsLinkPreview = false
        view.allowsBackForwardNavigationGestures = false
        #if DEBUG
        view.isInspectable = true
        #endif
        view.navigationDelegate = context.coordinator
        view.uiDelegate = context.coordinator
        context.coordinator.view = view
        context.coordinator.update(self)
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        context.coordinator.update(self)
    }

    static func dismantleUIView(_ view: WKWebView, coordinator: SurfaceWebCoordinator) {
        view.configuration.userContentController.removeAllScriptMessageHandlers()
        view.stopLoading()
        coordinator.view = nil
    }
}

/// SurfaceWebCoordinator loads an Html's page, carries what the page and
/// the app say to each other, and lets the page go nowhere.
@MainActor
final class SurfaceWebCoordinator: NSObject, WKNavigationDelegate, WKUIDelegate {
    /// The world the app listens to the page in, apart from the page's own
    /// scripts: they cannot reach the app's handler, nor change what it
    /// reads of the user's activation.
    static var world: WKContentWorld { .world(name: "kou") }

    /// listener runs in the app's world. It hands the app what the page's
    /// bridge posts, with whether the user is at work in the page: a click,
    /// a touch or a key of theirs in the last 5 s. Not the page's own
    /// navigator.userActivation: WebKit takes a script the app runs for a
    /// gesture of the user's, so the page's handlers of what the app sends
    /// would seem the user's. For the same reason what the app sends goes
    /// to the page through a MessageChannel, whose messages come without it.
    static let listener = """
    (() => {
      let input = -Infinity
      const mark = e => { if (e.isTrusted && !(e.type === 'keydown' && e.key === 'Escape')) input = performance.now() }
      for (const type of ['pointerdown', 'pointerup', 'mousedown', 'touchend', 'keydown', 'click']) addEventListener(type, mark, true)
      addEventListener('kou:post', e => {
        try { window.webkit.messageHandlers.kou.postMessage({ detail: String(e.detail), active: performance.now() - input < 5000 }) } catch (_) {}
      }, true)
      const channel = new MessageChannel()
      channel.port1.onmessage = e => dispatchEvent(new CustomEvent('kou:native', { detail: String(e.data) }))
      globalThis.__kouSend = m => channel.port2.postMessage(String(m))
    })()
    """

    weak var view: WKWebView?
    private var onMessage: ((SurfaceObject, Bool) -> Void)?
    /// The page loaded, or loading.
    private var html: String?
    /// The values and the theme the page is to have.
    private var data = "{}"
    private var theme = "{}"
    /// The values and the theme the page has.
    private var sentData: String?
    private var sentTheme: String?
    /// Whether the page said it is ready for what the app sends.
    private var ready = false
    /// Whether the navigation to come is the app's own loading of the page.
    private var loading = false
    /// How often the page's web content process ended, and the page was
    /// loaded anew.
    private var restarts = 0

    func update(_ web: SurfaceWebView) {
        onMessage = web.onMessage
        data = web.data
        theme = web.theme
        if web.html != html {
            html = web.html
            restarts = 0
            load()
        } else {
            push()
        }
    }

    private func load() {
        guard let view, let html else { return }
        ready = false
        sentData = nil
        sentTheme = nil
        loading = true
        view.loadHTMLString(html, baseURL: nil)
    }

    /// receive takes a message the page posted.
    func receive(_ message: WKScriptMessage) {
        guard message.frameInfo.isMainFrame, let body = message.body as? [String: Any],
              let detail = body["detail"] as? String, detail.utf8.count <= 64 << 10,
              let parsed = try? SurfaceJSON.parse(detail), let object = parsed.object,
              object.number("kou") == 1 else { return }
        let active = body["active"] as? Bool ?? false
        if object.string("type") == "ready" {
            ready = true
            sentData = nil
            sentTheme = nil
            push()
        }
        onMessage?(object, active)
    }

    /// push sends the page the values and the theme it does not have yet.
    private func push() {
        guard ready else { return }
        if sentData == nil || sentTheme == nil {
            send(#"{"kou":1,"type":"init","data":\#(data),"theme":\#(theme)}"#)
        } else {
            if data != sentData { send(#"{"kou":1,"type":"data","data":\#(data)}"#) }
            if theme != sentTheme { send(#"{"kou":1,"theme":\#(theme)}"#) }
        }
        sentData = data
        sentTheme = theme
    }

    /// send hands the page a message, from the app's world: see listener.
    private func send(_ json: String) {
        view?.callAsyncJavaScript("globalThis.__kouSend && globalThis.__kouSend(m)",
                                  arguments: ["m": json], in: nil, in: Self.world, completionHandler: nil)
    }

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        // The page loads once, from the app; it goes nowhere after.
        if loading, navigationAction.targetFrame?.isMainFrame == true {
            loading = false
            return .allow
        }
        return .cancel
    }

    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        nil
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        guard restarts < 3 else { return }
        restarts += 1
        load()
    }
}

/// SurfaceWebHandler hands the page's messages to the coordinator without
/// holding it: a user content controller holds its handlers.
@MainActor
final class SurfaceWebHandler: NSObject, WKScriptMessageHandler {
    private weak var coordinator: SurfaceWebCoordinator?

    init(_ coordinator: SurfaceWebCoordinator) {
        self.coordinator = coordinator
        super.init()
    }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        coordinator?.receive(message)
    }
}

/// SurfaceWebRules is the rule list by which an Html's web view loads
/// nothing off the device.
@MainActor
enum SurfaceWebRules {
    /// The rule list, once it is compiled.
    static var list: WKContentRuleList?
    private static var compiling: Task<Void, Never>?

    // A url-filter is a regular expression without alternation: a rule a
    // scheme.
    private static let rules = #"""
    [{"trigger":{"url-filter":"^https?://"},"action":{"type":"block"}},\#
    {"trigger":{"url-filter":"^wss?://"},"action":{"type":"block"}},\#
    {"trigger":{"url-filter":"^ftps?://"},"action":{"type":"block"}},\#
    {"trigger":{"url-filter":"^file:"},"action":{"type":"block"}}]
    """#

    /// prepare compiles the rule list, once.
    static func prepare() async {
        if let compiling { return await compiling.value }
        let task = Task {
            let store: WKContentRuleListStore? = WKContentRuleListStore.default()
            list = try? await store?.compileContentRuleList(forIdentifier: "kou-surface-offline",
                                                            encodedContentRuleList: rules)
        }
        compiling = task
        await task.value
    }
}

/// SurfaceWebPage readies the page the cockpit makes for a web view of the
/// app's: in the cockpit its frame's origin and headers keep it apart; here
/// the page itself says what it may load.
enum SurfaceWebPage {
    static let head = #"<meta name="viewport" content="width=device-width, initial-scale=1">"#
        + #"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; "#
        + #"style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; "#
        + #"base-uri 'none'; form-action 'none'">"#

    static func prepare(_ page: String) -> String {
        guard let charset = page.range(of: #"<meta charset="utf-8">"#, options: .caseInsensitive) else {
            return head + page
        }
        var page = page
        page.insert(contentsOf: head, at: charset.upperBound)
        return page
    }
}

/// SurfaceTheme is the app's look as the custom properties an Html's page
/// draws with: the names the web cockpit gives its pages, in the app's
/// colours.
///
/// They are opaque, as the web cockpit's are. iOS's greys — a fill, a
/// separator, a secondary label — let what is under them through, and a
/// page lays them on one another, a tile on the card and its text on the
/// tile: each step came out paler than on the web, and a page's muted text
/// read as disabled. So the greys stand from the card's ground as
/// theme.css's stand from its cards' (its bg-2), in WCAG contrast, made in
/// the card's own grey: lighter in dark and darker in light, with bg a
/// step below the card.
@MainActor
enum SurfaceTheme {
    /// How far theme.css's greys stand from its cards' ground, bg-2, in
    /// contrast: in dark, and in light.
    private static let ladder: [(name: String, dark: Double, light: Double)] = [
        ("--bg", 1.04, 1.06), ("--bg-3", 1.06, 1.14), ("--bg-4", 1.18, 1.27),
        ("--line", 1.24, 1.34), ("--line-2", 1.55, 1.69),
        ("--fg-2", 8.73, 9.21), ("--fg-3", 4.55, 4.43), ("--fg-4", 2.64, 2.57),
    ]

    static func json(dark: Bool) -> String {
        var environment = EnvironmentValues()
        environment.colorScheme = dark ? .dark : .light
        // The card's ground, as SurfaceBlock lays its fill on the screen's.
        let ground = laid(Color.kouGround, on: LookColor.RGB(r: 0, g: 0, b: 0), in: environment)
        let card = laid(Color(uiColor: .quaternarySystemFill), on: ground, in: environment)
        let cardHex = LookColor.hex(card)
        func solid(_ color: Color, _ alpha: Double = 1) -> SurfaceValue {
            .string(LookColor.hex(laid(color, on: card, alpha: alpha, in: environment)))
        }
        var vars = SurfaceObject()
        vars["--bg-2"] = .string(cardHex)
        for step in ladder {
            vars[step.name] = .string(LookColor.fit(cardHex, against: cardHex, contrast: dark ? step.dark : step.light,
                                                    lighter: dark && step.name != "--bg"))
        }
        let accent = Color.kouAccent
        let red = Color(uiColor: .systemRed)
        // The rest as theme.css has them, the soft ones laid on the card.
        vars["--fg"] = solid(Color(uiColor: .label))
        vars["--accent"] = solid(accent)
        vars["--accent-ink"] = solid(Color.kouAccentInk)
        vars["--accent-soft"] = solid(accent, dark ? 0.12 : 0.1)
        vars["--ok"] = solid(Color(uiColor: .systemGreen))
        vars["--warn"] = solid(Color(uiColor: .systemOrange))
        vars["--err"] = solid(red)
        vars["--err-soft"] = solid(red, dark ? 0.09 : 0.07)
        vars["--code"] = solid(Color.kouSurface)
        vars["--selection"] = solid(accent, dark ? 0.3 : 0.22)
        for (i, color) in SurfaceKinds.series.enumerated() {
            vars["--s\(i)"] = solid(color)
        }
        vars["--mono"] = .string("ui-monospace, SFMono-Regular, Menlo, monospace")
        vars["--sans"] = .string("-apple-system, system-ui, sans-serif")
        vars["--radius"] = .string(px(.corner(10)))
        vars["--radius-sm"] = .string(px(.corner(6)))
        vars["--radius-lg"] = .string(px(.corner(14)))
        let theme = SurfaceObject([("scheme", .string(dark ? "dark" : "light")), ("vars", .object(vars))])
        return SurfaceJSON.encode(.object(theme))
    }

    private static func px(_ value: CGFloat) -> String { "\(Int(value.rounded()))px" }

    /// laid is a colour as the screen shows it on an opaque ground: as far
    /// as the colour, times alpha, is opaque.
    private static func laid(_ color: Color, on ground: LookColor.RGB, alpha: Double = 1,
                             in environment: EnvironmentValues) -> LookColor.RGB {
        let c = color.resolve(in: environment)
        let a = min(max(Double(c.opacity) * alpha, 0), 1)
        func mix(_ v: Float, _ under: Double) -> Double { min(max(Double(v), 0), 1) * a + under * (1 - a) }
        return LookColor.RGB(r: mix(c.red, ground.r), g: mix(c.green, ground.g), b: mix(c.blue, ground.b))
    }
}
