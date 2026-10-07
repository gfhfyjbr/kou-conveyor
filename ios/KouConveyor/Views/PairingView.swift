import SwiftUI
import UIKit

/// PairingView pairs the app with a relay: the cockpit's /tunnel shows a
/// code to scan and a link to paste.
struct PairingView: View {
    @Environment(AppModel.self) private var model
    @State private var scanning = false
    @State private var manual = false
    @State private var busy = false
    @State private var error: String?
    @Namespace private var glass

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 28) {
                    hero
                    steps
                    if let error {
                        Label(error, systemImage: "exclamationmark.triangle.fill")
                            .font(.callout)
                            .foregroundStyle(.red)
                            .padding(14)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .glassEffect(.regular.tint(.red.opacity(0.12)), in: .rect(cornerRadius: 18))
                            .transition(.blurReplace)
                    }
                }
                .padding(.horizontal, 24)
                .padding(.top, 40)
                .padding(.bottom, 24)
                .frame(maxWidth: 560)
                .frame(maxWidth: .infinity)
            }
            .scrollBounceBehavior(.basedOnSize)
            .background(backdrop)
            .safeAreaBar(edge: .bottom) {
                actions
            }
            .sheet(isPresented: $scanning) {
                ScannerSheet { link in
                    scanning = false
                    pair(link)
                }
            }
            .sheet(isPresented: $manual) {
                ManualPairingSheet { config in
                    manual = false
                    pair(config)
                }
            }
        }
    }

    private var hero: some View {
        VStack(spacing: 18) {
            PlanetMark(lineWidth: 1.1)
                .foregroundStyle(Color.kouAccent)
                .frame(width: 92, height: 92)
                .padding(26)
                .glassEffect(.regular.tint(Color.kouAccent.opacity(0.10)), in: .circle)
            VStack(spacing: 6) {
                Text("kou-conveyor")
                    .font(.system(.largeTitle, design: .monospaced, weight: .bold))
                Text("Your cockpit, carried through its relay: the sessions, the chat, the canvases — here.")
                    .font(.body)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
        }
    }

    private var steps: some View {
        VStack(alignment: .leading, spacing: 14) {
            step(1, "In the cockpit, run **/tunnel**. If no relay is set up yet, the agent sets one up on a host of yours.")
            step(2, "The tunnel's dialog shows a pairing code and a link.")
            step(3, "Scan the code here, or with the Camera app, or paste the link.")
        }
        .padding(18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .glassEffect(.regular, in: .rect(cornerRadius: 22))
    }

    private func step(_ number: Int, _ text: LocalizedStringKey) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text("\(number)")
                .font(.system(.subheadline, design: .monospaced, weight: .bold))
                .foregroundStyle(Color.kouAccent)
                .frame(width: 18)
            Text(text)
                .font(.subheadline)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var actions: some View {
        GlassEffectContainer(spacing: 12) {
            VStack(spacing: 10) {
                Button {
                    error = nil
                    scanning = true
                } label: {
                    Label("Scan pairing code", systemImage: "qrcode.viewfinder")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.glassProminent)
                .controlSize(.large)
                .glassEffectID("scan", in: glass)

                HStack(spacing: 10) {
                    Button {
                        paste()
                    } label: {
                        Label("Paste link", systemImage: "doc.on.clipboard")
                            .frame(maxWidth: .infinity)
                    }
                    .glassEffectID("paste", in: glass)
                    Button {
                        error = nil
                        manual = true
                    } label: {
                        Label("Enter by hand", systemImage: "keyboard")
                            .frame(maxWidth: .infinity)
                    }
                    .glassEffectID("manual", in: glass)
                }
                .buttonStyle(.glass)
                .controlSize(.large)
            }
        }
        .disabled(busy)
        .overlay {
            if busy {
                ProgressView()
                    .controlSize(.large)
                    .padding(18)
                    .glassEffect(.regular, in: .circle)
            }
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 8)
    }

    private var backdrop: some View {
        ZStack {
            Color(uiColor: .systemBackground)
            RadialGradient(colors: [Color.kouAccent.opacity(0.28), .clear], center: .top, startRadius: 0, endRadius: 420)
            RadialGradient(colors: [Color.indigo.opacity(0.18), .clear], center: .bottomTrailing, startRadius: 0, endRadius: 380)
        }
        .ignoresSafeArea()
    }

    private func paste() {
        guard let text = UIPasteboard.general.string, !text.isEmpty else {
            withAnimation { error = "The clipboard has no link. In the cockpit, the tunnel's dialog copies it." }
            return
        }
        pair(text)
    }

    private func pair(_ link: String) {
        do {
            pair(try RelayConfig(link: link))
        } catch {
            withAnimation { self.error = error.localizedDescription }
        }
    }

    private func pair(_ config: RelayConfig) {
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                try await model.pair(config)
            } catch {
                withAnimation { self.error = error.localizedDescription }
            }
        }
    }
}

/// ManualPairingSheet takes a relay's address, token and fingerprint, as
/// `kou-conveyor-relay -print-config` prints them.
struct ManualPairingSheet: View {
    var done: (RelayConfig) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var url = ""
    @State private var token = ""
    @State private var fingerprint = ""
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("https://relay.example.com:8420", text: $url)
                        .keyboardType(.URL)
                        .textContentType(.URL)
                } header: {
                    Text("Relay")
                }
                Section {
                    SecureField("Token", text: $token)
                        .textContentType(.password)
                } header: {
                    Text("Token")
                } footer: {
                    Text("On the relay's host: kou-conveyor-relay -print-config")
                }
                Section {
                    TextField("SHA-256, 64 hex digits", text: $fingerprint, axis: .vertical)
                        .font(.system(.footnote, design: .monospaced))
                } header: {
                    Text("Certificate fingerprint")
                } footer: {
                    Text("For a relay with its own certificate (-tls self-signed, the default). Leave it empty for a certificate a CA signed, or for -tls off.")
                }
                if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                    }
                }
            }
            .autocorrectionDisabled()
            .textInputAutocapitalization(.never)
            .navigationTitle("Enter by hand")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Pair", systemImage: "checkmark") {
                        do {
                            done(try RelayConfig(url: url, token: token, fingerprint: fingerprint))
                        } catch {
                            self.error = error.localizedDescription
                        }
                    }
                    .buttonStyle(.glassProminent)
                    .disabled(url.isEmpty || token.isEmpty)
                }
            }
        }
    }
}
