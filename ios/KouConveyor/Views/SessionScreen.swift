import PhotosUI
import SwiftUI
import UIKit

/// SessionScreen is one session: its transcript as the agent writes it, and
/// the composer that runs it, with the model, the effort and the images of
/// the next prompt.
struct SessionScreen: View {
    @Environment(AppModel.self) private var app
    @State private var model: SessionModel
    @State private var draft: String
    @State private var attachments: [Attachment]
    @State private var sheet: Sheet?
    @State private var renaming = false
    @State private var title = ""
    @State private var editing: QueueItem?
    @State private var editText = ""
    @State private var pickingPhotos = false
    @State private var photos: [PhotosPickerItem] = []
    @State private var voice = VoiceInput()
    /// What the composer held as dictation started, and what dictation
    /// wrote in it last.
    @State private var dictation: (base: String, written: String)?
    /// The entry first in the transcript as earlier ones were asked for.
    @State private var kept: String?
    /// Whether the transcript's end is in view.
    @State private var following = true
    /// Whether the transcript is being scrolled, by the user or as it settles.
    @State private var scrolling = false
    /// Whether the transcript, once read, was taken to its end to be shown.
    @State private var placed = false
    /// The last entry as the user scrolled away from the end: what came
    /// after it is new. Earlier entries read meanwhile are not.
    @State private var seen: String?
    /// Opens another session: the one a branch makes.
    var open: (SessionRoute) -> Void

    enum Sheet: Identifiable {
        case models
        /// What a prompt changed; with none, the session's prompts to choose from.
        case changes(Entry?)
        case picture(path: String, title: String)
        /// A prompt, edited: the session runs again from it.
        case edit(Entry)

        var id: String {
            switch self {
            case .models: "models"
            case let .changes(entry): "changes:\(entry?.id ?? "")"
            case let .picture(path, _): "picture:\(path)"
            case let .edit(entry): "edit:\(entry.id)"
            }
        }
    }

    init(client: RelayClient, route: SessionRoute, open: @escaping (SessionRoute) -> Void) {
        _model = State(initialValue: SessionModel(client: client, workspace: route.workspace, id: route.id, fresh: route.fresh))
        _draft = State(initialValue: route.seed?.prompt ?? "")
        _attachments = State(initialValue: (route.seed?.images ?? []).compactMap(Attachment.init(image:)))
        self.open = open
    }

    var body: some View {
        screen
            .sheet(item: $sheet, content: sheetContent)
            .alert("Rename the session", isPresented: $renaming) {
                TextField("Title", text: $title)
                Button("Rename") {
                    let title = title
                    Task { await model.rename(title) }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Empty, it takes the first prompt for its title again.")
            }
            .alert("Edit the queued message", isPresented: editingShown, presenting: editing) { item in
                TextField("Message", text: $editText, axis: .vertical)
                Button("Save") {
                    let text = editText
                    Task { await model.change(item, QueueChange(text: text)) }
                }
                Button("Cancel", role: .cancel) {}
            }
            .task { await model.watch() }
            .task { _ = await app.loadCatalog(model.workspace) }
            .onDisappear {
                model.close()
                voice.cancel()
            }
            .sensoryFeedback(.success, trigger: model.phase) { old, new in old != .idle && new == .idle }
    }

    /// The transcript with its bars, title and menu: the body less its
    /// sheets and alerts, apart for the type checker's sake.
    private var screen: some View {
        transcript
            // The bar stands on the page's ground (BarBase): the system's
            // fade under it would only be drawn over again.
            .scrollEdgeEffectHidden(true, for: .bottom)
            .overlay { placeholder }
            .safeAreaBar(edge: .top) { noticeBar }
            .safeAreaBar(edge: .bottom) { bottomBar }
            .animation(.snappy, value: model.notice)
            .navigationTitle(model.displayTitle)
            .navigationBarTitleDisplayMode(.inline)
            .navigationSubtitle(subtitle)
            .toolbar(.hidden, for: .tabBar)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    sessionMenu
                }
            }
            .photosPicker(isPresented: $pickingPhotos, selection: $photos, maxSelectionCount: max(1, 20 - attachments.count),
                          matching: .images, preferredItemEncoding: .compatible)
            .onChange(of: photos) { _, items in
                guard !items.isEmpty else { return }
                photos = []
                Task { await attach(items) }
            }
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
        List {
            if model.earlier > 0 {
                earlierButton
                    .modifier(TranscriptRow())
            }
            let context = context
            let live = model.isBusy
            ForEach(model.entries) { entry in
                // Equatable: an entry that did not change is not drawn
                // again as the agent writes another, or a key is pressed.
                EntryView(entry: entry, live: live, context: context)
                    .equatable()
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .modifier(TranscriptRow())
            }
            runState
                .modifier(TranscriptRow())
            if !model.queued.isEmpty {
                QueueView(model: model) { item in
                    editText = item.text
                    editing = item
                }
                .modifier(TranscriptRow())
            }
            Color.clear
                .frame(height: 1)
                .modifier(TranscriptRow(vertical: 0))
                .id(Self.end)
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .environment(\.defaultMinListRowHeight, 0)
        .contentMargins(.vertical, 5, for: .scrollContent)
        // The run's row comes and goes softly; not the transcript read with it.
        .animation(placed ? .smooth(duration: 0.25) : nil, value: model.isBusy)
        .opacity(placed || model.entries.isEmpty ? 1 : 0)
        .task(id: model.entries.isEmpty) {
            // Should the list never tell it reached the end, it is shown
            // where it is.
            guard !model.entries.isEmpty, !placed else { return }
            try? await Task.sleep(for: .milliseconds(600))
            placed = true
        }
        .scrollDismissesKeyboard(.interactively)
        // Whether the end is in view: away from it, the page offers to jump
        // back, and counts what came meanwhile. Near it, the end stays in
        // view as the agent writes, the composer grows or the keyboard comes:
        // a list does not keep to its end by itself.
        .onScrollPhaseChange { _, phase in scrolling = phase.isScrolling }
        .onScrollGeometryChange(for: TranscriptGeometry.self, of: TranscriptGeometry.init) { old, new in
            // A session read is shown from its end: the list is laid out
            // from its top, and is not seen until it is at the end.
            if !placed, !model.entries.isEmpty {
                if new.pinned { placed = true } else { proxy.scrollTo(Self.end, anchor: .bottom) }
                return
            }
            // Under the user's finger the transcript goes where it is taken.
            if !scrolling, old.atEnd, !new.pinned, new.content != old.content || new.visible != old.visible {
                proxy.scrollTo(Self.end, anchor: .bottom)
                return
            }
            guard following != new.atEnd else { return }
            following = new.atEnd
            if new.atEnd { seen = nil } else if seen == nil { seen = model.entries.last?.id ?? "" }
        }
        .overlay(alignment: .bottom) {
            if !following, !model.entries.isEmpty {
                JumpButton(unseen: unseen) {
                    withAnimation(.smooth(duration: 0.35)) { proxy.scrollTo(Self.end, anchor: .bottom) }
                }
                // Above the fade of the bar's ground, not half in it.
                .padding(.bottom, BarBase.fade + 4)
                .transition(.scale(scale: 0.8).combined(with: .opacity))
            }
        }
        .animation(.snappy(duration: 0.25), value: following)
        // Once the page read is laid out above, what was first goes back to
        // the top: the screen stays on what was read.
        .onChange(of: model.entries.first?.id) {
            guard let kept else { return }
            self.kept = nil
            proxy.scrollTo(kept, anchor: .top)
        }
        }
    }

    /// How many entries came after the last one seen at the end.
    private var unseen: Int {
        guard let seen, let last = model.entries.lastIndex(where: { $0.id == seen }) else { return 0 }
        return model.entries.count - 1 - last
    }

    /// The id of the transcript's end, which the jump button scrolls to.
    private static let end = "transcript-end"

    /// The session is read from its end; this reads a page more of it.
    private var earlierButton: some View {
        Button {
            kept = model.entries.first?.id
            Task { await model.showEarlier() }
        } label: {
            HStack(spacing: 6) {
                if model.loadingEarlier {
                    ProgressView().controlSize(.small)
                } else {
                    Image(systemName: "chevron.up")
                }
                Text("Show earlier · \(model.earlier) more")
            }
            .font(.footnote.weight(.medium))
        }
        .buttonStyle(.bordered)
        .buttonBorderShape(.capsule)
        .disabled(model.loadingEarlier)
        .frame(maxWidth: .infinity)
    }

    @ViewBuilder
    private var runState: some View {
        if model.isBusy {
            ActivityRow(activity: model.activity, started: model.started, reconnecting: model.reconnecting)
                .transition(.blurReplace)
        } else if model.interrupted, !model.entries.isEmpty, !model.external {
            Label("The last run stopped before the agent finished.", systemImage: "pause.circle")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity)
        }
    }

    @ViewBuilder
    private var placeholder: some View {
        // Until the transcript read is shown from its end.
        if model.loading || !placed && !model.entries.isEmpty {
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

    @ViewBuilder
    private var noticeBar: some View {
        if let notice = model.notice {
            NoticeBanner(notice: notice) { model.notice = nil }
                .padding(.horizontal, 16)
                .transition(.move(edge: .top).combined(with: .opacity))
        }
    }

    private var bottomBar: some View {
        VStack(spacing: 8) {
            if !attachments.isEmpty {
                AttachmentStrip(attachments: attachments, remove: remove)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            ComposerOptions(
                modelTitle: nextModelTitle,
                chosen: model.chosenModel != nil,
                pickModel: { sheet = .models },
                notify: { model.notice = SessionModel.Notice(text: $0, isError: false) }
            )
            composer
        }
        .animation(.snappy, value: attachments.count)
        .background { BarBase() }
    }

    /// The composer, and dictation into it: apart for the type checker's sake.
    private var composer: some View {
        Composer(text: $draft, phase: model.phase, external: model.external,
                 images: attachments.count, send: send, stop: stop,
                 pickPhotos: { pickingPhotos = true }, paste: pasteImage,
                 listening: listening, dictate: dictate, dictated: dictation != nil)
            .onChange(of: voice.transcript) { _, words in write(dictated: words) }
            // Watched through its binding, not read here: read in this body,
            // the draft would draw the whole screen again at every key.
            .modifier(DraftWatch(text: $draft, edited: edited))
            .onChange(of: voice.failure) { _, failure in failed(failure) }
            .sensoryFeedback(trigger: voice.state, Self.feedback)
    }

    /// How dictation goes, nil where the cockpit does not take it.
    private var listening: VoiceInput.State? {
        dictates ? voice.state : nil
    }

    /// edited ends dictation once what it wrote is typed over, or sent.
    private func edited(_ text: String) {
        guard let current = dictation, text != current.written else { return }
        dictation = nil
        voice.cancel()
    }

    private func failed(_ failure: String?) {
        guard let failure else { return }
        model.notice = SessionModel.Notice(text: failure, isError: true)
    }

    /// What dictation feels like as it starts to listen, and as it ends.
    nonisolated private static func feedback(_ old: VoiceInput.State, _ new: VoiceInput.State) -> SensoryFeedback? {
        if new == .listening { return .start }
        return old == .finishing && new == .idle ? .stop : nil
    }

    @ViewBuilder
    private func sheetContent(_ sheet: Sheet) -> some View {
        switch sheet {
        case .models:
            ModelPicker(workspace: model.workspace, chosen: model.chosenModel, last: model.lastModel) { id in
                choose(id)
            }
            .presentationDetents([.medium, .large])
        case let .changes(entry):
            ChangesSheet(client: model.client, workspace: model.workspace, session: model.id,
                         prompts: prompts, start: entry)
        case let .picture(path, title):
            PictureViewer(client: model.client, path: path, title: title)
        case let .edit(entry):
            PromptEditor(entry: entry, after: max(0, model.replaced(by: entry) - 1)) { text in
                await model.edit(entry, to: text, effort: app.knownEffort)
            }
            .presentationDetents([.medium, .large])
        }
    }

    /// The prompts the user sent that the session has: what changes are
    /// asked of.
    private var prompts: [Entry] {
        (model.earlierPrompts + model.entries).filter { $0.kind == Entry.user && $0.messageID != nil && $0.state == nil }
    }

    private var nextModelTitle: String {
        app.modelTitle(model.nextModel ?? app.defaultModel(model.workspace), in: model.workspace)
    }

    private var editingShown: Binding<Bool> {
        Binding(get: { editing != nil }, set: { if !$0 { editing = nil } })
    }

    private var context: EntryContext {
        EntryContext(
            imagePath: { entry, n in model.imagePath(entry, n) },
            client: model.client,
            branch: { branch($0) },
            changes: { sheet = .changes($0) },
            picture: { path, title in sheet = .picture(path: path, title: title) },
            edit: model.phase == .idle && !model.external ? { sheet = .edit($0) } : nil
        )
    }

    private var known: Bool { !(model.fresh && model.entries.isEmpty) }

    private var sessionMenu: some View {
        Menu {
            if known {
                Section {
                    Button("Rename…", systemImage: "pencil") {
                        title = model.title ?? ""
                        renaming = true
                    }
                    Button(model.pinned ? "Unpin" : "Pin", systemImage: model.pinned ? "pin.slash" : "pin") {
                        Task { await model.setPinned(!model.pinned) }
                    }
                    Button("Duplicate", systemImage: "plus.square.on.square") { branch(nil) }
                        .disabled(model.isBusy)
                    Button("Changes", systemImage: "plusminus.circle") { sheet = .changes(nil) }
                }
            }
            Section {
                Button("Model: \(app.modelTitle(model.nextModel ?? app.defaultModel(model.workspace), in: model.workspace))",
                       systemImage: "cpu") { sheet = .models }
            }
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
        let sending = attachments
        let images = sending.filter { text.contains($0.label) }.map(\.prompt)
        draft = ""
        attachments = []
        Task {
            let sent = await model.send(text, force: force, images: images, effort: app.knownEffort)
            // What was not sent is back in the composer, unless the user
            // wrote something else meanwhile.
            if !sent, draft.isEmpty {
                draft = text
                if attachments.isEmpty { attachments = sending }
            }
        }
    }

    private func stop() {
        Task { await model.stop() }
    }

    /// Whether the cockpit takes dictation here: Soniox's key is set.
    private var dictates: Bool {
        app.workspaces.first { $0.id == model.workspace }?.voice == true
    }

    /// dictate starts dictation, or stops it: what is said goes after what
    /// the composer holds.
    private func dictate() {
        switch voice.state {
        case .idle:
            dictation = (draft, draft)
            voice.start(client: model.client, workspace: model.workspace)
        case .starting, .listening:
            voice.stop()
        case .finishing:
            break
        }
    }

    /// write puts the words said so far after what the composer held.
    private func write(dictated words: String) {
        guard let current = dictation, draft == current.written else { return }
        let base = current.base
        let gap = base.isEmpty || words.isEmpty || base.last?.isWhitespace == true ? "" : " "
        let text = base + gap + words
        dictation = (base, text)
        draft = text
    }

    private func choose(_ id: String?) {
        model.choose(model: id)
        let shown = id ?? model.lastModel ?? app.defaultModel(model.workspace)
        let info = app.catalog(model.workspace)?.find(shown)
        let name = app.modelTitle(shown, in: model.workspace)
        let provider = info?.providerName.map { " · \($0)" } ?? ""
        let rest = model.isBusy ? " — the running agent keeps its own model" : ""
        model.notice = SessionModel.Notice(text: "Next prompts: \(name)\(provider)\(rest)", isError: false)
    }

    // MARK: Branches

    private func branch(_ entry: Entry?) {
        Task {
            guard let result = await model.branch(before: entry) else { return }
            var images: [PromptImage] = []
            if let entry { images = await model.images(of: entry) }
            let seed = result.prompt.flatMap { $0.isEmpty ? nil : SessionRoute.Seed(prompt: $0, images: images) }
            if let id = result.sessionID, !id.isEmpty {
                open(SessionRoute(workspace: model.workspace, id: id, fresh: false, seed: seed))
            } else {
                open(SessionRoute(workspace: model.workspace, id: UUID().uuidString.lowercased(), fresh: true, seed: seed))
            }
        }
    }

    // MARK: Images

    private func nextLabel() -> String {
        let used = attachments.compactMap { Entry.ImageInfo(label: $0.label).number }
        return "[Image \((used.max() ?? 0) + 1)]"
    }

    private func add(_ processed: Attachment.Processed) {
        guard attachments.count < 20 else {
            model.notice = SessionModel.Notice(text: "A prompt takes at most 20 images.", isError: true)
            return
        }
        let label = nextLabel()
        attachments.append(Attachment(label: label, processed: processed))
        let separator = draft.isEmpty || draft.hasSuffix(" ") || draft.hasSuffix("\n") ? "" : " "
        draft += separator + label + " "
    }

    private func attach(_ items: [PhotosPickerItem]) async {
        for item in items {
            guard let data = try? await item.loadTransferable(type: Data.self) else {
                model.notice = SessionModel.Notice(text: "Could not read the picture.", isError: true)
                continue
            }
            guard let processed = await Task.detached(operation: { Attachment.process(data) }).value else {
                model.notice = SessionModel.Notice(text: "That picture cannot be sent.", isError: true)
                continue
            }
            add(processed)
        }
    }

    private func pasteImage() {
        guard let image = UIPasteboard.general.image, let data = image.pngData() else {
            model.notice = SessionModel.Notice(text: "There is no picture to paste.", isError: false)
            return
        }
        Task {
            if let processed = await Task.detached(operation: { Attachment.process(data) }).value { add(processed) }
        }
    }

    private func remove(_ attachment: Attachment) {
        attachments.removeAll { $0.id == attachment.id }
        draft = draft.replacingOccurrences(of: attachment.label + " ", with: "")
            .replacingOccurrences(of: attachment.label, with: "")
    }
}

// MARK: Images

/// An image that goes with the prompt being written, which names it by its
/// label: [Image 1].
struct Attachment: Identifiable, Hashable {
    let id = UUID()
    let label: String
    let mediaType: String
    let data: Data
    let preview: UIImage?

    /// An image made ready to send: within the sides and bytes a model takes.
    struct Processed: Sendable {
        let data: Data
        let mediaType: String
        let thumbnail: Data?
    }

    init(label: String, processed: Processed) {
        self.label = label
        mediaType = processed.mediaType
        data = processed.data
        preview = processed.thumbnail.flatMap(UIImage.init(data:))
    }

    /// An image a branch's prompt had.
    init?(image: PromptImage) {
        guard let data = Data(base64Encoded: image.data) else { return nil }
        label = image.label
        mediaType = image.mediaType
        self.data = data
        preview = UIImage(data: data)?.preparingThumbnail(of: CGSize(width: 160, height: 160))
    }

    var prompt: PromptImage {
        PromptImage(label: label, mediaType: mediaType, data: data.base64EncodedString())
    }

    /// process makes a picture ready to send: at most 2000 pixels a side
    /// and 3.5 MB, as JPEG.
    nonisolated static func process(_ data: Data) -> Processed? {
        guard let image = UIImage(data: data) else { return nil }
        let pixels = CGSize(width: image.size.width * image.scale, height: image.size.height * image.scale)
        let longest = max(pixels.width, pixels.height)
        guard longest > 0 else { return nil }
        let factor = min(1, 2000 / longest)
        let target = CGSize(width: max(1, (pixels.width * factor).rounded()), height: max(1, (pixels.height * factor).rounded()))
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = true
        let scaled = UIGraphicsImageRenderer(size: target, format: format).image { context in
            UIColor.white.setFill()
            context.fill(CGRect(origin: .zero, size: target))
            image.draw(in: CGRect(origin: .zero, size: target))
        }
        var quality: CGFloat = 0.85
        guard var bytes = scaled.jpegData(compressionQuality: quality) else { return nil }
        while bytes.count > 3_500_000, quality > 0.35 {
            quality -= 0.15
            bytes = scaled.jpegData(compressionQuality: quality) ?? bytes
        }
        let thumbnail = scaled.preparingThumbnail(of: CGSize(width: 160, height: 160 * target.height / target.width))
        return Processed(data: bytes, mediaType: "image/jpeg", thumbnail: thumbnail?.jpegData(compressionQuality: 0.8))
    }
}

/// AttachmentStrip shows the images the prompt goes with, each removable.
struct AttachmentStrip: View {
    let attachments: [Attachment]
    var remove: (Attachment) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 10) {
                ForEach(attachments) { attachment in
                    VStack(spacing: 4) {
                        Group {
                            if let preview = attachment.preview {
                                Image(uiImage: preview)
                                    .resizable()
                                    .scaledToFill()
                            } else {
                                Image(systemName: "photo")
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .frame(width: 58, height: 58)
                        .clipShape(.rect(cornerRadius: 12))
                        .overlay(alignment: .topTrailing) {
                            Button {
                                remove(attachment)
                            } label: {
                                Image(systemName: "xmark.circle.fill")
                                    .symbolRenderingMode(.palette)
                                    .foregroundStyle(.white, .black.opacity(0.6))
                                    .font(.body)
                            }
                            .buttonStyle(.plain)
                            .offset(x: 6, y: -6)
                            .accessibilityLabel("Remove \(attachment.label)")
                        }
                        Text(attachment.label.dropFirst().dropLast())
                            .font(.caption2.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
        }
    }
}

// MARK: The composer's options

/// ComposerOptions are the choices of the next prompt, above the field: its
/// model, the effort, the tools and the sandbox of the next runs.
struct ComposerOptions: View {
    @Environment(AppModel.self) private var app
    let modelTitle: String
    /// A model was chosen for the session, not left to its default.
    let chosen: Bool
    var pickModel: () -> Void
    var notify: (String) -> Void

    var body: some View {
        GlassEffectContainer(spacing: 8) {
            HStack(spacing: 8) {
                Button(action: pickModel) {
                    HStack(spacing: 5) {
                        Image(systemName: "cpu")
                        Text(modelTitle)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Image(systemName: "chevron.up.chevron.down")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.secondary)
                    }
                    .font(.footnote.weight(.medium))
                    .foregroundStyle(chosen ? Color.kouAccent : .primary)
                }
                .buttonStyle(.glass)
                .accessibilityLabel("Model: \(modelTitle)")

                Menu {
                    Picker("Effort", selection: effortChoice) {
                        ForEach(app.effortLevels, id: \.self) { level in
                            Label(Effort.title(level), systemImage: Effort.symbol(level, in: app.effortLevels))
                                .tag(level)
                        }
                    }
                } label: {
                    HStack(spacing: 5) {
                        Image(systemName: Effort.symbol(app.effort, in: app.effortLevels))
                        Text(Effort.title(app.effort))
                            .lineLimit(1)
                    }
                    .font(.footnote.weight(.medium))
                }
                .buttonStyle(.glass)
                .accessibilityLabel("Effort: \(Effort.title(app.effort))")

                Spacer(minLength: 0)

                Menu {
                    if let profiles = app.preferences?.toolProfiles, !profiles.isEmpty {
                        Menu {
                            choices(profiles, current: app.toolProfile ?? "auto") { value in
                                save(PreferencesChange(toolProfile: value), "Tools for the next runs: \(value)")
                            }
                        } label: {
                            Label("Tools: \(app.toolProfile ?? "auto")", systemImage: "wrench.and.screwdriver")
                        }
                    }
                    if let sandboxes = app.preferences?.sandboxes, !sandboxes.isEmpty {
                        Menu {
                            choices(sandboxes, current: app.sandbox ?? "off") { value in
                                save(PreferencesChange(sandbox: value), "Sandbox for the next runs: \(value)")
                            }
                        } label: {
                            Label("Sandbox: \(app.sandbox ?? "off")", systemImage: "shippingbox")
                        }
                    }
                    Button("Reload the settings", systemImage: "arrow.clockwise") {
                        Task { await app.loadSettings() }
                    }
                } label: {
                    Image(systemName: "slider.horizontal.3")
                        .font(.footnote.weight(.semibold))
                }
                .buttonStyle(.glass)
                .accessibilityLabel("Tools and sandbox")
            }
        }
        .padding(.horizontal, 12)
    }

    @ViewBuilder
    private func choices(_ options: [Preferences.Choice], current: String, pick: @escaping (String) -> Void) -> some View {
        ForEach(options) { option in
            Button {
                pick(option.value)
            } label: {
                if option.value == current {
                    Label {
                        Text(option.value)
                        if let description = option.description { Text(description) }
                    } icon: {
                        Image(systemName: "checkmark")
                    }
                } else {
                    Text(option.value)
                    if let description = option.description { Text(description) }
                }
            }
        }
    }

    /// The effort of the next runs, saved as it is chosen. (A closure: the
    /// method itself as the setter crashes the compiler.)
    private var effortChoice: Binding<String> {
        Binding(get: { app.effort }, set: { level in save(effort: level) })
    }

    private func save(effort level: String) {
        save(PreferencesChange(effort: level), "Effort of the next runs: \(Effort.title(level))")
    }

    private func save(_ change: PreferencesChange, _ done: String) {
        Task {
            if let error = await app.save(change) {
                notify("Not saved: \(error)")
            } else {
                notify(done)
            }
        }
    }
}

/// Effort reads the levels of effort as the cockpit's page shows them.
enum Effort {
    static func title(_ level: String) -> String {
        switch level {
        case "low": "Low"
        case "medium": "Medium"
        case "high": "High"
        case "xhigh": "Extra high"
        case "max": "Max"
        default: level
        }
    }

    /// A gauge filled as far as the level goes among the levels.
    static func symbol(_ level: String, in levels: [String]) -> String {
        guard let index = levels.firstIndex(of: level), levels.count > 1 else { return "gauge.with.dots.needle.50percent" }
        let part = Double(index) / Double(levels.count - 1)
        switch part {
        case ..<0.17: return "gauge.with.dots.needle.0percent"
        case ..<0.42: return "gauge.with.dots.needle.33percent"
        case ..<0.59: return "gauge.with.dots.needle.50percent"
        case ..<0.84: return "gauge.with.dots.needle.67percent"
        default: return "gauge.with.dots.needle.100percent"
        }
    }
}

/// BarBase is what the composer stands on: the page's own ground, from a
/// little above the bar down to the screen's foot, as the cockpit's dock
/// stands below its transcript. The glass of the controls takes its shade
/// and its shadow from what passes under it, a moment late: over the text
/// of a transcript being scrolled they lagged and flickered.
struct BarBase: View {
    /// How far above the bar the transcript fades out.
    static let fade: CGFloat = 18

    var body: some View {
        let ground = Color(uiColor: .systemBackground)
        VStack(spacing: 0) {
            LinearGradient(colors: [ground.opacity(0), ground], startPoint: .top, endPoint: .bottom)
                .frame(height: Self.fade)
            ground
        }
        .padding(.top, -Self.fade)
        .ignoresSafeArea(edges: .bottom)
        .allowsHitTesting(false)
    }
}

/// TranscriptGeometry is what the transcript keeps to its end by: how tall
/// it is, how much of it is read at once, and how near its end what is read is.
private struct TranscriptGeometry: Equatable {
    var content: CGFloat
    var visible: CGFloat
    /// The end is near enough that the transcript follows it.
    var atEnd: Bool
    /// The end is in view.
    var pinned: Bool

    init(_ geometry: ScrollGeometry) {
        // The visible rect runs under the bars; the composer's bar is not
        // where the transcript is read.
        let bottom = geometry.visibleRect.maxY - geometry.contentInsets.bottom
        let gap = geometry.contentSize.height - bottom
        content = geometry.contentSize.height
        visible = geometry.containerSize.height - geometry.contentInsets.top - geometry.contentInsets.bottom
        atEnd = gap <= 120
        pinned = gap <= 8
    }
}

/// TranscriptRow draws a row of the transcript's list as the page does: no
/// separators, no background, 14 points between rows.
private struct TranscriptRow: ViewModifier {
    var vertical: CGFloat = 7

    func body(content: Content) -> some View {
        content
            .listRowInsets(EdgeInsets(top: vertical, leading: 16, bottom: vertical, trailing: 16))
            .listRowSeparator(.hidden)
            .listRowBackground(Color.clear)
    }
}

/// DraftWatch tells of the composer's text as it changes, where only it
/// depends on the text.
private struct DraftWatch: ViewModifier {
    @Binding var text: String
    var edited: (String) -> Void

    func body(content: Content) -> some View {
        content.onChange(of: text) { _, text in edited(text) }
    }
}

/// JumpButton takes the transcript back to its end, as the page's "N new ↓".
struct JumpButton: View {
    let unseen: Int
    var jump: () -> Void

    var body: some View {
        Button(action: jump) {
            HStack(spacing: 5) {
                if unseen > 0 {
                    Text("\(unseen) new")
                        .contentTransition(.numericText())
                }
                Image(systemName: "arrow.down")
            }
            .font(.footnote.weight(.semibold))
            .padding(.horizontal, unseen > 0 ? 14 : 11)
            .padding(.vertical, 9)
            // All the capsule takes the tap, not its arrow alone.
            .contentShape(.capsule)
        }
        .buttonStyle(.plain)
        .glassEffect(.regular.interactive(), in: .capsule)
        .animation(.snappy, value: unseen)
        .accessibilityLabel(unseen > 0 ? "\(unseen) new entries, jump to the latest" : "Jump to the latest")
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

/// QueueView is what waits for the agent: messages sent while it worked,
/// each to put to it now, edit, move or drop.
struct QueueView: View {
    let model: SessionModel
    var edit: (QueueItem) -> Void

    var body: some View {
        let items = model.queued
        let paused = model.queue?.paused == true
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Label(paused ? "Queue paused" : "Queued · \(items.count)", systemImage: paused ? "pause.circle" : "tray.full")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                Spacer()
                Menu {
                    if paused {
                        Button("Resume the queue", systemImage: "play.fill") { Task { await model.resumeQueue() } }
                    } else {
                        Button("Pause the queue", systemImage: "pause.fill") { Task { await model.pauseQueue() } }
                    }
                    Button("Clear the queue", systemImage: "trash", role: .destructive) { Task { await model.clearQueue() } }
                } label: {
                    Image(systemName: "ellipsis.circle")
                        .font(.subheadline)
                }
                .accessibilityLabel("Queue")
            }
            ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: item.forced == true ? "bolt.fill" : "clock")
                        .font(.caption)
                        .foregroundStyle(item.forced == true ? Color.kouAccent : .secondary)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(item.text)
                            .font(.subheadline)
                            .lineLimit(3)
                        if let images = item.images, !images.isEmpty {
                            Label("\(images.count) image\(images.count == 1 ? "" : "s")", systemImage: "photo")
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                        }
                    }
                    Spacer(minLength: 0)
                    if item.forced != true {
                        Menu {
                            actions(item, index: index, count: items.count)
                        } label: {
                            Image(systemName: "ellipsis")
                                .font(.footnote.weight(.semibold))
                                .frame(width: 28, height: 22)
                                .contentShape(.rect)
                        }
                        .foregroundStyle(.secondary)
                    }
                }
                .contentShape(.rect)
                .contextMenu {
                    if item.forced != true { actions(item, index: index, count: items.count) }
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 16))
    }

    @ViewBuilder
    private func actions(_ item: QueueItem, index: Int, count: Int) -> some View {
        Button("Put to the agent now", systemImage: "bolt.fill") {
            Task { await model.change(item, QueueChange(force: true)) }
        }
        Button("Edit…", systemImage: "pencil") { edit(item) }
        if index > 0 {
            Button("Move up", systemImage: "arrow.up") {
                Task { await model.change(item, QueueChange(position: index - 1)) }
            }
        }
        if index < count - 1 {
            Button("Move down", systemImage: "arrow.down") {
                Task { await model.change(item, QueueChange(position: index + 1)) }
            }
        }
        Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = item.text }
        Divider()
        Button("Drop", systemImage: "trash", role: .destructive) {
            Task { await model.drop(item) }
        }
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
    let phase: SessionModel.Phase
    let external: Bool
    /// The images the prompt goes with.
    let images: Int
    var send: (_ force: Bool) -> Void
    var stop: () -> Void
    var pickPhotos: () -> Void
    var paste: () -> Void
    /// How dictation goes, nil where the cockpit does not take it; dictate
    /// starts and stops it.
    var listening: VoiceInput.State? = nil
    var dictate: () -> Void = {}
    /// Whether what the composer holds is what dictation wrote: the
    /// microphone stays for more of it, and goes once the user types.
    var dictated = false
    @Namespace private var glass

    // Stops at the first letter: a long text is not copied to say so.
    private var empty: Bool { !text.contains { !$0.isWhitespace } }
    private var working: Bool { phase == .running || phase == .stopping }

    /// The microphone is there while the composer is empty, or holds what
    /// was said, and while dictation goes.
    private var mic: Bool {
        guard let listening, !external else { return false }
        return listening != .idle || empty || dictated
    }

    var body: some View {
        GlassEffectContainer(spacing: 10) {
            HStack(alignment: .bottom, spacing: 10) {
                Menu {
                    Button("Photo Library", systemImage: "photo.on.rectangle", action: pickPhotos)
                    Button("Paste Image", systemImage: "doc.on.clipboard", action: paste)
                } label: {
                    Image(systemName: "plus")
                        .font(.body.weight(.semibold))
                        .frame(width: 26, height: 30)
                }
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .disabled(external || images >= 20)
                .glassEffectID("attach", in: glass)
                .accessibilityLabel("Add an image")

                ComposerField(text: $text, placeholder: placeholder, editable: !external)
                    .frame(minHeight: 46)
                    .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 23))
                    .glassEffectID("field", in: glass)

                if mic, let listening {
                    Button(action: dictate) {
                        Image(systemName: listening == .listening ? "waveform" : "mic")
                            .font(.body.weight(.semibold))
                            .foregroundStyle(listening == .idle ? Color.primary : Color.kouAccent)
                            .symbolEffect(.variableColor.iterative, isActive: listening == .listening)
                            .symbolEffect(.pulse, isActive: listening == .starting || listening == .finishing)
                            .frame(width: 26, height: 30)
                    }
                    .buttonStyle(.glass)
                    .buttonBorderShape(.circle)
                    .disabled(listening == .finishing)
                    .glassEffectID("mic", in: glass)
                    .accessibilityLabel(listening == .idle ? "Dictate" : "Stop dictating")
                }

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
            .animation(.smooth(duration: 0.3), value: mic)
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 8)
    }

    private var placeholder: String {
        if external { return "Running in another window" }
        return working ? "Queue a message for the agent" : "Message the agent"
    }
}

/// ComposerField is the composer's text: a UITextView that grows from one
/// line to seven and scrolls past them.
///
/// SwiftUI's vertical TextField finds its height by laying out all of its
/// text, at every change: a long paste froze the screen for a third of a
/// second at a time. This field lays out only what it shows, and measures
/// a text only while it is short enough to fit — a longer one is as tall as
/// the field gets.
struct ComposerField: UIViewRepresentable {
    @Binding var text: String
    let placeholder: String
    let editable: Bool

    /// The most lines the field grows to.
    static let maxLines = 7
    /// Past this many characters a text cannot fit in fewer lines than the
    /// most, at any width the composer has: it is not measured.
    static let measured = 1500
    static let inset = UIEdgeInsets(top: 11, left: 16, bottom: 11, right: 16)

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> UITextView {
        // TextKit 1: TextKit 2 estimates the height of a long text it has
        // not laid out, and its caret and its content's end disagree — a
        // long paste left the caret out of reach, and the field crept to it
        // over many frames.
        let view = ComposerTextView.make()
        view.delegate = context.coordinator
        view.backgroundColor = .clear
        view.font = .preferredFont(forTextStyle: .body)
        view.adjustsFontForContentSizeCategory = true
        view.textColor = .label
        view.textContainerInset = Self.inset
        view.textContainer.lineFragmentPadding = 0
        view.isScrollEnabled = true
        view.showsVerticalScrollIndicator = false
        view.alwaysBounceVertical = false
        // Pasted text comes as text, not with the fonts and colours of
        // where it was copied from.
        view.allowsEditingTextAttributes = false
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.setContentHuggingPriority(.defaultLow, for: .horizontal)
        view.accessibilityLabel = placeholder
        context.coordinator.placeholder = view.placeholder

        view.text = text
        context.coordinator.written = text
        update(view, coordinator: context.coordinator)
        return view
    }

    func updateUIView(_ view: UITextView, context: Context) {
        context.coordinator.parent = self
        // What the binding holds is set when it changed elsewhere — by
        // dictation, by sending — not while a word is being composed. It is
        // told from what the field last wrote, not from the view's own text,
        // which comes bridged and compares slowly, a character at a time.
        let coordinator = context.coordinator
        if text != coordinator.written, view.markedTextRange == nil {
            view.text = text
            coordinator.written = text
        }
        update(view, coordinator: coordinator)
        if !editable, view.isFirstResponder { view.resignFirstResponder() }
    }

    private func update(_ view: UITextView, coordinator: Coordinator) {
        if view.isEditable != editable { view.isEditable = editable }
        // Its colour is set on all of the text: only when it changes.
        let color: UIColor = editable ? .label : .secondaryLabel
        if view.textColor != color { view.textColor = color }
        if view.accessibilityLabel != placeholder { view.accessibilityLabel = placeholder }
        if coordinator.placeholder?.text != placeholder { coordinator.placeholder?.text = placeholder }
        coordinator.placeholder?.isHidden = view.hasText
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView view: UITextView, context: Context) -> CGSize? {
        guard let width = proposal.width, width.isFinite, width > 0 else { return nil }
        let font = view.font ?? .preferredFont(forTextStyle: .body)
        let line = font.lineHeight
        let vertical = Self.inset.top + Self.inset.bottom
        let most = ceil(line * CGFloat(Self.maxLines) + vertical)
        let least = ceil(line + vertical)
        let length = view.textStorage.length
        let height: CGFloat
        if length == 0 {
            height = least
        } else if length > Self.measured {
            height = most
        } else {
            let available = max(1, width - Self.inset.left - Self.inset.right)
            let text = view.textStorage.string
            // A last empty line, after a newline, counts too.
            let measured = text.hasSuffix("\n") ? text + " " : text
            let rect = (measured as NSString).boundingRect(
                with: CGSize(width: available, height: .greatestFiniteMagnitude),
                options: [.usesLineFragmentOrigin, .usesFontLeading],
                attributes: [.font: font],
                context: nil)
            height = min(most, max(least, ceil(rect.height) + vertical))
        }
        return CGSize(width: width, height: height)
    }

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: ComposerField
        weak var placeholder: UILabel?
        /// What the field last wrote to the binding, or was set to.
        var written: String

        init(_ parent: ComposerField) {
            self.parent = parent
            written = parent.text
        }

        func textViewDidChange(_ view: UITextView) {
            placeholder?.isHidden = view.hasText
            // A Swift string of its own, not the view's NSString: SwiftUI
            // compares the draft with the last one wherever it goes, and a
            // bridged string of a long paste compares at a fraction of the
            // speed, normalizing every character, at every key.
            var text = view.textStorage.string
            text.makeContiguousUTF8()
            written = text
            parent.text = text
        }
    }
}

/// The composer's text view: as it grows, the caret stays in view.
///
/// Its placeholder is placed by hand: a constraint in it would put it under
/// Auto Layout, which asks its baseline at every change of size — and the
/// baseline of a text view is found by laying out all of its text, again
/// at every frame the field's width animates.
final class ComposerTextView: UITextView {
    let placeholder: UILabel = {
        let label = UILabel()
        label.font = .preferredFont(forTextStyle: .body)
        label.adjustsFontForContentSizeCategory = true
        label.textColor = .placeholderText
        label.numberOfLines = 1
        label.isUserInteractionEnabled = false
        return label
    }()
    private var shownHeight: CGFloat = 0
    /// The text, kept here: its layout manager does not keep it.
    private var storage: NSTextStorage?

    /// A text view on TextKit 1, made through its designated initializer —
    /// init(usingTextLayoutManager:) makes it by a way that skips this
    /// class's own properties.
    static func make() -> ComposerTextView {
        let storage = NSTextStorage()
        let manager = NSLayoutManager()
        storage.addLayoutManager(manager)
        let container = NSTextContainer(size: CGSize(width: 0, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = true
        manager.addTextContainer(container)
        let view = ComposerTextView(frame: .zero, textContainer: container)
        view.storage = storage
        return view
    }

    override init(frame: CGRect, textContainer: NSTextContainer?) {
        super.init(frame: frame, textContainer: textContainer)
        addSubview(placeholder)
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        addSubview(placeholder)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        if !placeholder.isHidden {
            let inset = textContainerInset
            let height = ceil(placeholder.font.lineHeight)
            placeholder.frame = CGRect(x: inset.left, y: inset.top,
                                       width: max(0, bounds.width - inset.left - inset.right), height: height)
        }
        guard bounds.height != shownHeight else { return }
        shownHeight = bounds.height
        if isFirstResponder { scrollRangeToVisible(selectedRange) }
    }
}
