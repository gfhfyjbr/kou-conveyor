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
                    ContentUnavailableView {
                        Label("No camera", systemImage: "camera.badge.ellipsis")
                    } description: {
                        Text(problem)
                    }
                    .foregroundStyle(.white)
                } else {
                    QRScannerView(found: found) { problem = $0 }
                        .ignoresSafeArea()
                    RoundedRectangle(cornerRadius: 28)
                        .strokeBorder(.white.opacity(0.85), style: StrokeStyle(lineWidth: 3, dash: [26, 14]))
                        .frame(width: 250, height: 250)
                        .allowsHitTesting(false)
                }
            }
            .safeAreaBar(edge: .bottom) {
                Text("Point the camera at the code in the tunnel's dialog: /tunnel in the cockpit.")
                    .font(.footnote)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 18)
                    .padding(.vertical, 12)
                    .glassEffect(.regular, in: .capsule)
                    .padding()
            }
            .navigationTitle("Pairing code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
            }
        }
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
