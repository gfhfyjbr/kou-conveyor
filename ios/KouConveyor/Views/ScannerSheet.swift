@preconcurrency import AVFoundation
import SwiftUI
import UIKit

/// ScannerSheet reads the pairing code the cockpit's /tunnel shows.
struct ScannerSheet: View {
    var found: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var problem: String?

    var body: some View {
        NavigationStack {
            ZStack {
                Color.black.ignoresSafeArea()
                if let problem {
                    KouEmpty(symbol: "camera.badge.ellipsis", title: "No camera", message: problem)
                } else {
                    QRScannerView(found: found) { problem = $0 }
                    .ignoresSafeArea()
                    // The camera dimmed but for the square the code is read
                    // in, both drawn over the whole screen: their middles
                    // are the same.
                    ZStack {
                        FinderMask(size: Viewfinder.size, radius: .corner(26))
                            .fill(.black.opacity(0.5), style: FillStyle(eoFill: true))
                        Viewfinder()
                    }
                    .ignoresSafeArea()
                    .allowsHitTesting(false)
                }
            }
            .safeAreaBar(edge: .bottom) {
                VStack(spacing: 6) {
                    Eyebrow("Scan to pair", color: .kouFg3)
                    // U+2060: the command whole, not broken after its slash.
                    Text("Point the camera at the code in the tunnel's dialog: `/\u{2060}tunnel` in the cockpit.")
                        .font(.footnote)
                        .foregroundStyle(Color.kouFg)
                        .multilineTextAlignment(.center)
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 14)
                .glassEffect(.regular, in: .rect(cornerRadius: .corner(22)))
                .padding()
            }
            .navigationTitle("Pairing code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
                }
            }
        }
        // A camera's screen is dark whatever the phone is.
        .preferredColorScheme(.dark)
    }
}

/// Viewfinder is the square the code is read in, as a camera draws it: its
/// four corners, and the accent's line sweeping it while the camera looks.
struct Viewfinder: View {
    static let size: CGFloat = 250
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var down = false

    var body: some View {
        ZStack {
            FinderCorners(length: 38, radius: .corner(26))
                .stroke(.white, style: StrokeStyle(lineWidth: 4, lineCap: .round, lineJoin: .round))
            if !reduceMotion {
                Capsule()
                    .fill(LinearGradient(colors: [Color.kouAccent.opacity(0), Color.kouAccent, Color.kouAccent.opacity(0)],
                                         startPoint: .leading, endPoint: .trailing))
                    .frame(height: 2)
                    .shadow(color: Color.kouAccent.opacity(0.9), radius: 6)
                    .padding(.horizontal, 22)
                    .offset(y: (down ? 1 : -1) * (Self.size / 2 - 30))
                    .onAppear {
                        withAnimation(.easeInOut(duration: 1.7).repeatForever(autoreverses: true)) { down = true }
                    }
            }
        }
        .frame(width: Self.size, height: Self.size)
    }
}

/// FinderCorners are a viewfinder's four corners: each a length of either
/// side, round where they meet.
struct FinderCorners: Shape {
    var length: CGFloat
    var radius: CGFloat

    func path(in rect: CGRect) -> Path {
        var path = Path()
        let r = min(radius, length)
        // Each corner: where it is, and which way its sides go from it.
        let corners: [(CGPoint, CGFloat, CGFloat)] = [
            (CGPoint(x: rect.minX, y: rect.minY), 1, 1),
            (CGPoint(x: rect.maxX, y: rect.minY), -1, 1),
            (CGPoint(x: rect.maxX, y: rect.maxY), -1, -1),
            (CGPoint(x: rect.minX, y: rect.maxY), 1, -1),
        ]
        for (corner, dx, dy) in corners {
            path.move(to: CGPoint(x: corner.x, y: corner.y + dy * length))
            path.addLine(to: CGPoint(x: corner.x, y: corner.y + dy * r))
            path.addQuadCurve(to: CGPoint(x: corner.x + dx * r, y: corner.y), control: corner)
            path.addLine(to: CGPoint(x: corner.x + dx * length, y: corner.y))
        }
        return path
    }
}

/// FinderMask is the screen with the viewfinder's square cut out of its
/// middle, to be filled even-odd.
struct FinderMask: Shape {
    let size: CGFloat
    let radius: CGFloat

    func path(in rect: CGRect) -> Path {
        var path = Path(rect)
        let hole = CGRect(x: rect.midX - size / 2, y: rect.midY - size / 2, width: size, height: size)
        path.addRoundedRect(in: hole, cornerSize: CGSize(width: radius, height: radius), style: .continuous)
        return path
    }
}

/// QRScannerView shows the camera, and gives the first pairing link it
/// reads in a QR code.
struct QRScannerView: UIViewControllerRepresentable {
    var found: (String) -> Void
    var failed: (String) -> Void

    func makeUIViewController(context: Context) -> ScannerController {
        let controller = ScannerController()
        controller.found = found
        controller.failed = failed
        return controller
    }

    func updateUIViewController(_ controller: ScannerController, context: Context) {
        controller.found = found
        controller.failed = failed
    }
}

/// The capture session, which runs on a queue of its own: starting it
/// blocks.
private final class Capture: @unchecked Sendable {
    let session = AVCaptureSession()
    let queue = DispatchQueue(label: "kou-conveyor.scanner")

    func start() { queue.async { self.session.startRunning() } }
    func stop() { queue.async { self.session.stopRunning() } }
}

final class ScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var found: ((String) -> Void)?
    var failed: ((String) -> Void)?
    private let capture = Capture()
    private var preview: AVCaptureVideoPreviewLayer?
    private var done = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            configure()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { granted in
                Task { @MainActor [weak self] in
                    if granted { self?.configure() } else { self?.failed?("The camera is not allowed. Allow it in Settings, or paste the link instead.") }
                }
            }
        default:
            failed?("The camera is not allowed. Allow it in Settings, or paste the link instead.")
        }
    }

    private func configure() {
        let session = capture.session
        guard let device = AVCaptureDevice.default(for: .video), let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input)
        else {
            failed?("This device has no camera to scan with. Paste the link instead.")
            return
        }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else {
            failed?("The camera cannot read codes here. Paste the link instead.")
            return
        }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]
        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.videoGravity = .resizeAspectFill
        layer.frame = view.bounds
        view.layer.addSublayer(layer)
        preview = layer
        capture.start()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        preview?.frame = view.bounds
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        capture.stop()
    }

    nonisolated func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput metadataObjects: [AVMetadataObject], from connection: AVCaptureConnection) {
        let values = metadataObjects.compactMap { ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }
        // The delegate is called on the main queue (configure says so).
        MainActor.assumeIsolated {
            guard !done, let link = values.first(where: { (try? RelayConfig(link: $0)) != nil }) else { return }
            done = true
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            capture.stop()
            found?(link)
        }
    }
}
