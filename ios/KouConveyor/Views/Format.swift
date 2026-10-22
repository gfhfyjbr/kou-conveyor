import Foundation
import SwiftUI
import UIKit

/// Format reads numbers as the cockpit's page does: tokens in a few
/// characters, money in dollars or the user's credits.
enum Format {
    /// "950", "48.2K", "1.06B".
    static func tokens(_ n: Int64?) -> String {
        guard let n else { return "—" }
        if abs(n) >= 1_000_000_000 {
            let value = Double(n) / 1e9
            return value.formatted(.number.precision(.fractionLength(abs(n) < 10_000_000_000 ? 2 : 1))) + "B"
        }
        return compact(Double(n))
    }

    /// "2,880", "10,963", "1.2M".
    static func count(_ n: Int64?) -> String {
        guard let n else { return "—" }
        return abs(n) < 100_000 ? n.formatted(.number) : compact(Double(n))
    }

    /// "1.2K", "15", "0.42".
    static func compact(_ value: Double) -> String {
        value.formatted(.number.notation(.compactName).precision(.significantDigits(1...3)))
    }

    static func isDollars(_ unit: MoneyUnit?) -> Bool {
        guard let unit else { return true }
        return unit.name == "USD" || !((unit.usd ?? 0) > 0)
    }

    /// An amount of dollars in the unit: "$1,234.56", "$0.042",
    /// "12,346 credits".
    static func money(_ usd: Double?, unit: MoneyUnit? = nil) -> String {
        guard let usd, usd.isFinite else { return "—" }
        let dollars = isDollars(unit)
        let value = dollars ? usd : usd / (unit?.usd ?? 1)
        let size = abs(value)
        let sign = value < 0 ? "−" : ""
        if dollars {
            if size > 0, size < 0.001 { return "\(sign)<$0.001" }
            let digits = size < 0.1 ? 3 : size < 10_000 ? 2 : 0
            let text = size == 0 ? "0" : size.formatted(.number.precision(.fractionLength(digits)))
            return "\(sign)$\(text)"
        }
        let text: String
        if size == 0 {
            text = "0"
        } else if size < 1 {
            text = size.formatted(.number.precision(.significantDigits(1...2)))
        } else {
            text = size.formatted(.number.precision(.fractionLength(0...(size < 100 ? 1 : 0))))
        }
        return "\(sign)\(text) \(unit?.name ?? "")"
    }

    /// How much a context window holds: "200k", "1M".
    static func context(_ tokens: Int?) -> String? {
        guard let tokens, tokens > 0 else { return nil }
        if tokens >= 1_000_000 {
            let millions = Double(tokens) / 1e6
            return millions.formatted(.number.precision(.fractionLength(tokens % 1_000_000 == 0 ? 0 : 1))) + "M"
        }
        return "\(Int((Double(tokens) / 1000).rounded()))k"
    }

    static func bytes(_ n: Int64?) -> String {
        guard let n else { return "" }
        return ByteCountFormatter.string(fromByteCount: n, countStyle: .file)
    }

    /// "850 ms", "1.2 s".
    static func latency(_ ms: Double?) -> String {
        guard let ms, ms > 0 else { return "—" }
        if ms < 1000 { return "\(Int(ms.rounded())) ms" }
        return (ms / 1000).formatted(.number.precision(.fractionLength(1))) + " s"
    }

    /// The change from before to now: "+12%", "−3%"; nil with nothing before.
    static func change(_ now: Double, from before: Double) -> String? {
        guard before > 0 else { return nil }
        let percent = (now - before) / before * 100
        guard abs(percent) >= 0.5 else { return "±0%" }
        return (percent > 0 ? "+" : "−") + "\(Int(abs(percent).rounded()))%"
    }

    /// How long something took, as the page's fmt.duration: "0.4s", "12s",
    /// "3m 05s", "1h 02m".
    static func duration(_ seconds: TimeInterval) -> String {
        guard seconds.isFinite, seconds >= 0 else { return "" }
        if seconds < 1 { return String(format: "%.1fs", seconds) }
        let s = Int(seconds)
        if s < 60 { return "\(s)s" }
        if s < 3600 { return String(format: "%dm %02ds", s / 60, s % 60) }
        return String(format: "%dh %02dm", s / 3600, s / 60 % 60)
    }

    /// When something was said: the time today, the day and time before.
    static func time(_ date: Date) -> String {
        if Calendar.current.isDateInToday(date) {
            return date.formatted(date: .omitted, time: .shortened)
        }
        return date.formatted(.dateTime.month(.abbreviated).day().hour().minute())
    }
}

// MARK: Pictures

/// RemoteImage shows a picture the cockpit serves, fetched through the relay
/// with its token, which AsyncImage cannot send.
struct RemoteImage: View {
    let client: RelayClient
    let path: String
    var query: [URLQueryItem] = []
    var contentMode: ContentMode = .fill
    /// Whether a picture not there yet is asked for again: the one a tool
    /// call read comes a moment after the call, once the session file has it.
    var retryMissing = false
    @State private var image: UIImage?
    @State private var failed = false
    /// The picture the state is of: another one's is not shown meanwhile.
    @State private var shown: String?

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .aspectRatio(contentMode: contentMode)
            } else if failed {
                Image(systemName: "photo.badge.exclamationmark")
                    .font(.title3)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(placeholder)
            } else {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(placeholder)
            }
        }
        .task(id: key) { await load() }
    }

    /// A thumbnail keeps its place while it loads.
    @ViewBuilder private var placeholder: some View {
        if contentMode == .fill {
            Rectangle().fill(.fill.quaternary)
        }
    }

    private var key: String {
        Self.key(path, query)
    }

    static func key(_ path: String, _ query: [URLQueryItem]) -> String {
        query.isEmpty ? path : path + "?" + query.map { "\($0.name)=\($0.value ?? "")" }.joined(separator: "&")
    }

    private func load() async {
        let key = key
        if shown == key, image != nil { return }
        if shown != key {
            shown = key
            image = nil
        }
        failed = false
        do {
            let (picture, fetched) = try await Self.fetch(client: client, path: path, query: query, retryMissing: retryMissing)
            guard key == self.key else { return }
            if fetched {
                withAnimation(.easeOut(duration: 0.2)) { image = picture }
            } else {
                image = picture
            }
        } catch is CancellationError {
        } catch {
            if !Task.isCancelled { failed = true }
        }
    }

    /// fetch reads a picture, from the cache when it has it; it says
    /// whether it came through the relay. A picture lost to the network is
    /// asked for twice more before it fails; with retryMissing, one the
    /// cockpit does not have yet too.
    static func fetch(client: RelayClient, path: String, query: [URLQueryItem],
                      retryMissing: Bool = false) async throws -> (UIImage, fetched: Bool) {
        let key = Self.key(path, query) as NSString
        if let cached = PictureCache.shared.object(forKey: key) { return (cached, false) }
        var attempt = 0
        while true {
            do {
                let data = try await client.perform(client.request("GET", path, query: query))
                guard let decoded = UIImage(data: data) else { throw PictureError.unreadable }
                // Decoded off the main thread, so that a list of them scrolls.
                let picture = await decoded.byPreparingForDisplay() ?? decoded
                let pixels = picture.size.width * picture.scale * picture.size.height * picture.scale
                PictureCache.shared.setObject(picture, forKey: key, cost: Int(pixels) * 4)
                return (picture, true)
            } catch let error as RelayError where attempt < 2 && (error.isTransient || retryMissing && error.isMissing) {
                attempt += 1
                try await Task.sleep(for: .seconds(attempt))
            }
        }
    }

    enum PictureError: LocalizedError {
        case unreadable
        var errorDescription: String? { "This file is not a picture this iPhone reads." }
    }
}

extension RelayError {
    /// A failure that asking again may not meet: the network, or the
    /// relay's gateway between it and the cockpit.
    var isTransient: Bool {
        switch self {
        case .network: true
        case let .http(status, _, _): status == 502 || status == 503 || status == 504
        default: false
        }
    }

    /// Not there: what the cockpit does not have yet, or any more.
    var isMissing: Bool {
        if case .http(404, _, _) = self { true } else { false }
    }
}

/// The pictures fetched, decoded, kept while memory allows.
@MainActor
enum PictureCache {
    static let shared: NSCache<NSString, UIImage> = {
        let cache = NSCache<NSString, UIImage>()
        cache.totalCostLimit = 160 << 20
        return cache
    }()
}

/// PictureViewer shows a picture whole, to zoom into as Photos does: pinch
/// or tap twice where to look.
struct PictureViewer: View {
    let client: RelayClient
    let path: String
    var query: [URLQueryItem] = []
    let title: String
    @Environment(\.dismiss) private var dismiss
    @State private var image: UIImage?
    @State private var problem: String?

    var body: some View {
        NavigationStack {
            ZStack {
                Color.black.ignoresSafeArea()
                if let image {
                    ZoomableImage(image: image)
                        .ignoresSafeArea()
                        .accessibilityLabel(title)
                } else if let problem {
                    ContentUnavailableView {
                        Label("The picture did not load", systemImage: "photo.badge.exclamationmark")
                    } description: {
                        Text(problem)
                    } actions: {
                        Button("Try again") { Task { await load() } }
                            .buttonStyle(.glass)
                    }
                    .environment(\.colorScheme, .dark)
                } else {
                    ProgressView()
                        .tint(.white)
                }
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbarColorScheme(.dark, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
                if let image {
                    ToolbarItem(placement: .primaryAction) {
                        let picture = Image(uiImage: image)
                        ShareLink(item: picture, preview: SharePreview(title, image: picture))
                    }
                }
            }
        }
        .task { await load() }
    }

    private func load() async {
        problem = nil
        do {
            image = try await RemoteImage.fetch(client: client, path: path, query: query).0
        } catch is CancellationError {
        } catch {
            problem = error.localizedDescription
        }
    }
}

/// ZoomableImage is a picture in a scroll view that zooms it: around the
/// pinch, to where it was tapped twice, kept in the middle while smaller
/// than the screen.
struct ZoomableImage: UIViewRepresentable {
    let image: UIImage

    func makeUIView(context: Context) -> ZoomingView {
        ZoomingView()
    }

    func updateUIView(_ view: ZoomingView, context: Context) {
        view.show(image)
    }

    final class ZoomingView: UIScrollView, UIScrollViewDelegate {
        private let picture = UIImageView()
        /// The size the picture was fitted to: it is fitted anew when the
        /// screen turns.
        private var fitted: CGSize = .zero

        override init(frame: CGRect) {
            super.init(frame: frame)
            delegate = self
            showsVerticalScrollIndicator = false
            showsHorizontalScrollIndicator = false
            contentInsetAdjustmentBehavior = .never
            decelerationRate = .fast
            bouncesZoom = true
            picture.isAccessibilityElement = false
            addSubview(picture)
            let twice = UITapGestureRecognizer(target: self, action: #selector(tappedTwice(_:)))
            twice.numberOfTapsRequired = 2
            addGestureRecognizer(twice)
        }

        required init?(coder: NSCoder) {
            nil
        }

        func show(_ image: UIImage) {
            guard picture.image !== image else { return }
            picture.image = image
            fitted = .zero
            setNeedsLayout()
        }

        override func layoutSubviews() {
            super.layoutSubviews()
            guard let image = picture.image, image.size.width > 0, image.size.height > 0,
                  bounds.width > 0, bounds.height > 0 else { return }
            if fitted != bounds.size {
                fitted = bounds.size
                zoomScale = 1
                // Fitted whole, a small picture at most twice its size.
                let fit = min(bounds.width / image.size.width, bounds.height / image.size.height, 2)
                let size = CGSize(width: image.size.width * fit, height: image.size.height * fit)
                picture.frame = CGRect(origin: .zero, size: size)
                contentSize = size
                minimumZoomScale = 1
                // Up to twice its own pixels: four times at least, ten at most.
                maximumZoomScale = min(max(4, 2 * image.size.width * image.scale / max(size.width, 1)), 10)
                center()
                contentOffset = CGPoint(x: -contentInset.left, y: -contentInset.top)
                return
            }
            center()
        }

        /// center keeps a picture smaller than the screen in its middle.
        private func center() {
            let size = picture.frame.size
            let x = max((bounds.width - size.width) / 2, 0)
            let y = max((bounds.height - size.height) / 2, 0)
            let inset = UIEdgeInsets(top: y, left: x, bottom: y, right: x)
            if contentInset != inset { contentInset = inset }
        }

        func viewForZooming(in scrollView: UIScrollView) -> UIView? {
            picture
        }

        func scrollViewDidZoom(_ scrollView: UIScrollView) {
            center()
        }

        @objc private func tappedTwice(_ tap: UITapGestureRecognizer) {
            if zoomScale > minimumZoomScale + 0.01 {
                setZoomScale(minimumZoomScale, animated: true)
                return
            }
            let point = tap.location(in: picture)
            let scale = min(maximumZoomScale, 2.5)
            let size = CGSize(width: bounds.width / scale, height: bounds.height / scale)
            zoom(to: CGRect(x: point.x - size.width / 2, y: point.y - size.height / 2,
                            width: size.width, height: size.height), animated: true)
        }
    }
}
