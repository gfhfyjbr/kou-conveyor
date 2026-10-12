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
    @State private var image: UIImage?
    @State private var failed = false

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
            } else {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task(id: key) { await load() }
    }

    private var key: String {
        query.isEmpty ? path : path + "?" + query.map { "\($0.name)=\($0.value ?? "")" }.joined(separator: "&")
    }

    private func load() async {
        if let cached = PictureCache.shared.object(forKey: key as NSString) {
            image = cached
            return
        }
        do {
            let data = try await client.perform(client.request("GET", path, query: query))
            guard let picture = UIImage(data: data) else {
                failed = true
                return
            }
            PictureCache.shared.setObject(picture, forKey: key as NSString, cost: data.count)
            image = picture
        } catch {
            if !(error is CancellationError) { failed = true }
        }
    }
}

/// The pictures fetched, kept while memory allows.
@MainActor
enum PictureCache {
    static let shared: NSCache<NSString, UIImage> = {
        let cache = NSCache<NSString, UIImage>()
        cache.totalCostLimit = 64 << 20
        return cache
    }()
}

/// PictureViewer shows a picture whole, to zoom into.
struct PictureViewer: View {
    let client: RelayClient
    let path: String
    var query: [URLQueryItem] = []
    let title: String
    @Environment(\.dismiss) private var dismiss
    @State private var zoom: CGFloat = 1
    @GestureState private var pinch: CGFloat = 1

    var body: some View {
        NavigationStack {
            GeometryReader { geometry in
                ScrollView([.horizontal, .vertical], showsIndicators: false) {
                    RemoteImage(client: client, path: path, query: query, contentMode: .fit)
                        .frame(width: geometry.size.width * zoom * pinch, height: geometry.size.height * zoom * pinch)
                }
                .defaultScrollAnchor(.center)
            }
            .gesture(
                MagnifyGesture()
                    .updating($pinch) { value, state, _ in state = value.magnification }
                    .onEnded { value in zoom = min(max(zoom * value.magnification, 1), 6) }
            )
            .onTapGesture(count: 2) { withAnimation(.snappy) { zoom = zoom > 1 ? 1 : 2.5 } }
            .background(Color.black)
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
            }
        }
    }
}
