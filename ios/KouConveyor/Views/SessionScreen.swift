import SwiftUI
import UIKit

/// SessionScreen is one session: its transcript as the agent writes it, and
/// the composer that runs it.
struct SessionScreen: View {
    @Environment(AppModel.self) private var app
    @State private var model: SessionModel
    @State private var draft = ""
    @FocusState private var focused: Bool

    init(client: RelayClient, route: SessionRoute) {
        _model = State(initialValue: SessionModel(client: client, workspace: route.workspace, id: route.id, fresh: route.fresh))
    }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 14) {
                ForEach(model.entries) { entry in
                    EntryView(entry: entry, live: model.isBusy)
                        .id(entry.id)
                }
                if model.isBusy {
                    ActivityRow(activity: model.activity, started: model.started, reconnecting: model.reconnecting)
                        .transition(.blurReplace)
                } else if model.interrupted, !model.entries.isEmpty, !model.external {
                    Label("The last run stopped before the agent finished.", systemImage: "pause.circle")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity)
                }
                if !model.queued.isEmpty {
                    QueueView(items: model.queued, paused: model.queue?.paused == true)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .animation(.smooth(duration: 0.25), value: model.isBusy)
        }
        .defaultScrollAnchor(.bottom)
        .defaultScrollAnchor(.bottom, for: .sizeChanges)
        .scrollDismissesKeyboard(.interactively)
        .overlay {
            if model.loading {
                ProgressView()
            } else if let error = model.loadError, model.entries.isEmpty {
                ContentUnavailableView("Session unavailable", systemImage: "exclamationmark.bubble", description: Text(error))
            } else if model.fresh, model.entries.isEmpty {
                ContentUnavailableView {
                    Label("New session", systemImage: "sparkles")
                } description: {
                    Text("The agent runs on the computer the cockpit runs on, in \(app.workspace?.name ?? "the workspace").")
                }
            }
        }
        .safeAreaBar(edge: .top) {
            if let notice = model.notice {
                NoticeBanner(notice: notice) { model.notice = nil }
                    .padding(.horizontal, 16)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .safeAreaBar(edge: .bottom) {
            Composer(text: $draft, focused: $focused, phase: model.phase, external: model.external, send: send, stop: stop)
        }
        .animation(.snappy, value: model.notice)
        .navigationTitle(model.displayTitle)
        .navigationBarTitleDisplayMode(.inline)
        .navigationSubtitle(subtitle)
        .toolbar(.hidden, for: .tabBar)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    if let usage = model.usage {
                        Section("Usage") {
                            Text("Input \(tokens(usage.input)) · cached \(tokens(usage.cached))")
                            Text("Output \(tokens(usage.output))")
                            if let context = usage.context, context > 0 { Text("Context \(tokens(context))") }
                        }
                    }
                    Button("Copy session ID", systemImage: "doc.on.doc") {
                        UIPasteboard.general.string = model.id
                    }
                } label: {
                    Label("Session", systemImage: "ellipsis")
                }
            }
        }
        .task { await model.watch() }
        .onDisappear { model.close() }
        .sensoryFeedback(.success, trigger: model.phase) { old, new in old != .idle && new == .idle }
    }

    private var subtitle: String {
        if model.external { return "Running in another window" }
        switch model.phase {
        case .idle: return app.workspace?.name ?? ""
        case .starting: return "Starting…"
        case .running: return model.activity ?? "Working"
        case .stopping: return "Stopping…"
        }
    }

    private func tokens(_ value: Int64?) -> String {
        (value ?? 0).formatted(.number.notation(.compactName))
    }

    private func send(force: Bool) {
        let text = draft
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        draft = ""
        Task {
            let sent = await model.send(text, force: force)
            // What was not sent is back in the composer, unless the user
            // wrote something else meanwhile.
            if !sent, draft.isEmpty { draft = text }
        }
    }

    private func stop() {
        Task { await model.stop() }
    }
}

/// ActivityRow says what the agent does while a run goes on.
struct ActivityRow: View {
    let activity: String?
    let started: Date?
    let reconnecting: Bool

    var body: some View {
        HStack(spacing: 10) {
            ProgressView()
                .controlSize(.small)
            Text(reconnecting ? "Reconnecting…" : (activity ?? "Working"))
                .font(.subheadline.weight(.medium))
                .lineLimit(1)
                .contentTransition(.opacity)
            if let started {
                Text(started, style: .timer)
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .glassEffect(.regular.tint(Color.kouAccent.opacity(0.12)), in: .capsule)
        .animation(.smooth, value: activity)
    }
}

/// QueueView is what waits for the agent: messages sent while it worked.
struct QueueView: View {
    let items: [QueueItem]
    let paused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(paused ? "Queue paused" : "Queued", systemImage: paused ? "pause.circle" : "tray.full")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            ForEach(items) { item in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: item.forced == true ? "bolt.fill" : "clock")
                        .font(.caption)
                        .foregroundStyle(item.forced == true ? Color.kouAccent : .secondary)
                    Text(item.text)
                        .font(.subheadline)
                        .lineLimit(3)
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 16))
    }
}

/// NoticeBanner is a word for the user over the transcript, which goes by
/// itself.
struct NoticeBanner: View {
    let notice: SessionModel.Notice
    var dismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: notice.isError ? "exclamationmark.triangle.fill" : "info.circle.fill")
                .foregroundStyle(notice.isError ? .red : Color.kouAccent)
            Text(notice.text)
                .font(.footnote)
                .lineLimit(3)
            Spacer(minLength: 0)
            Button("Dismiss", systemImage: "xmark", action: dismiss)
                .labelStyle(.iconOnly)
                .font(.footnote)
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassEffect(.regular, in: .rect(cornerRadius: 18))
        .task(id: notice.id) {
            try? await Task.sleep(for: .seconds(notice.isError ? 7 : 4))
            if !Task.isCancelled { dismiss() }
        }
    }
}

/// Composer is where the user writes to the agent: while it works, what is
/// sent waits in the queue; held, the send button puts it to the agent at
/// once.
struct Composer: View {
    @Binding var text: String
    var focused: FocusState<Bool>.Binding
    let phase: SessionModel.Phase
    let external: Bool
    var send: (_ force: Bool) -> Void
    var stop: () -> Void
    @Namespace private var glass

    private var empty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    private var working: Bool { phase == .running || phase == .stopping }

    var body: some View {
        GlassEffectContainer(spacing: 10) {
            HStack(alignment: .bottom, spacing: 10) {
                TextField(placeholder, text: $text, axis: .vertical)
                    .lineLimit(1...7)
                    .focused(focused)
                    .disabled(external)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 11)
                    .frame(minHeight: 46)
                    .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 23))
                    .glassEffectID("field", in: glass)

                if working {
                    Button(action: stop) {
                        Image(systemName: phase == .stopping ? "hourglass" : "stop.fill")
                            .font(.body.weight(.semibold))
                            .frame(width: 26, height: 30)
                    }
                    .buttonStyle(.glass)
                    .buttonBorderShape(.circle)
                    .disabled(phase == .stopping)
                    .glassEffectID("stop", in: glass)
                    .accessibilityLabel("Stop the run")
                }

                if !working || !empty {
                    Button {
                        send(false)
                    } label: {
                        Image(systemName: working ? "text.badge.plus" : "arrow.up")
                            .font(.body.weight(.bold))
                            .frame(width: 26, height: 30)
                    }
                    .buttonStyle(.glassProminent)
                    .buttonBorderShape(.circle)
                    .disabled(empty || external || phase == .starting)
                    .glassEffectID("send", in: glass)
                    .accessibilityLabel(working ? "Queue the message" : "Send")
                    .contextMenu {
                        if working {
                            Button("Put to the agent now", systemImage: "bolt.fill") { send(true) }
                            Button("Queue for after this run", systemImage: "text.badge.plus") { send(false) }
                        }
                    }
                }
            }
            .animation(.smooth(duration: 0.3), value: working)
            .animation(.smooth(duration: 0.3), value: empty)
        }
        .padding(.horizontal, 12)
        .padding(.top, 6)
        .padding(.bottom, 8)
    }

    private var placeholder: String {
        if external { return "Running in another window" }
        return working ? "Queue a message for the agent" : "Message the agent"
    }
}
