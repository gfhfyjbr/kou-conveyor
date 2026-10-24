import SwiftUI

/// ModelPicker chooses the model of a session's next prompts among those its
/// workspace reaches — through the accounts gateway, every model of every
/// account — grouped by provider, as the cockpit's page has them.
struct ModelPicker: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    let workspace: String
    /// The model chosen for the session in this app; nil: its own.
    let chosen: String?
    /// The model the session ran its last prompt with.
    let last: String?
    var choose: (String?) -> Void
    @State private var query = ""
    @State private var loading = false
    @State private var error: String?

    private struct Group: Identifiable {
        let name: String
        /// Its provider's key, which its mark is coloured by.
        var provider: String?
        var models: [ModelInfo] = []
        /// Models that make pictures or sound rather than answers: shown
        /// only when searched for.
        var media: [ModelInfo] = []

        var id: String { name }
    }

    var body: some View {
        NavigationStack {
            List {
                SwiftUI.Group {
                    if words.isEmpty {
                        Section {
                            defaultRow
                        } footer: {
                            FormNote(verbatim: footer)
                        }
                    }
                    ForEach(groups) { group in
                        let shown = (group.models + (words.isEmpty ? [] : group.media)).filter { matches($0) }
                        if !shown.isEmpty {
                            Section {
                                ForEach(shown) { row($0) }
                            } header: {
                                head(group, count: shown.count)
                            }
                        } else if words.isEmpty, !group.media.isEmpty {
                            Section {
                                Text("Only models that make media: search for one by name.")
                                    .font(.footnote)
                                    .foregroundStyle(Color.kouFg3)
                            } header: {
                                head(group, count: nil)
                            }
                        }
                    }
                    if let typed, catalog?.find(typed) == nil {
                        Section {
                            Button {
                                pick(typed)
                            } label: {
                                ActionLabel("Use “\(typed)”", symbol: "character.cursor.ibeam", tone: .kouAccent)
                            }
                        } footer: {
                            FormNote("A model the list does not have, by its ID: the provider decides whether it takes it.")
                        }
                    }
                }
                .kouRow()
            }
            .groupedBackground()
            .listSectionSpacing(22)
            .overlay {
                if catalog == nil {
                    if let error {
                        KouEmpty(symbol: "cpu", title: "Models unavailable", message: error, tone: .kouErr)
                    } else {
                        LoadingSpinner()
                    }
                }
            }
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Find a model")
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .refreshable { await reload(fresh: true) }
            .task { await reload(fresh: false) }
            .navigationTitle("Model")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                        .tint(Color.kouFg)
                }
                ToolbarItem(placement: .primaryAction) {
                    if loading {
                        ProgressView()
                            .tint(Color.kouFg3)
                    } else {
                        Button("Ask the providers again", systemImage: "arrow.clockwise") {
                            Task { await reload(fresh: true) }
                        }
                        .tint(Color.kouFg)
                    }
                }
            }
        }
    }

    private var catalog: ModelCatalog? { app.catalog(workspace) }

    /// A provider's head: its mark in its colour, its name in the cockpit's
    /// capitals, how many of its models show.
    private func head(_ group: Group, count: Int?) -> some View {
        HStack(spacing: 8) {
            ProviderMark(provider: group.provider ?? group.name, size: 18)
            ListHead(group.name, count: count)
        }
    }

    private var defaultRow: some View {
        Button {
            pick(nil)
        } label: {
            HStack(spacing: 12) {
                RowIcon("sparkles", tone: chosen == nil ? .kouAccent : .kouFg2)
                VStack(alignment: .leading, spacing: 3) {
                    Text(last != nil ? "As the session ran last" : "The workspace's default")
                        .foregroundStyle(Color.kouFg)
                    Text(app.modelTitle(last ?? app.defaultModel(workspace), in: workspace))
                        .font(.kouMeta)
                        .foregroundStyle(Color.kouFg3)
                }
                Spacer(minLength: 8)
                mark(chosen == nil)
            }
            .contentShape(.rect)
        }
        .accessibilityAddTraits(chosen == nil ? .isSelected : [])
    }

    private func row(_ model: ModelInfo) -> some View {
        Button {
            pick(model.id)
        } label: {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: model.title)
                        .foregroundStyle(Color.kouFg)
                    // What tells of it, in mono, as the cockpit's list has
                    // its ID and its window.
                    let detail = detail(model)
                    if !detail.isEmpty {
                        Text(verbatim: detail)
                            .font(.kouMeta)
                            .foregroundStyle(Color.kouFg3)
                            .lineLimit(2)
                    }
                }
                Spacer(minLength: 8)
                if model.cooling == true {
                    Image(systemName: "snowflake")
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(Color.kouWarn)
                        .accessibilityLabel("Cooling down")
                }
                mark(chosen == model.id)
            }
            .contentShape(.rect)
        }
        .accessibilityAddTraits(chosen == model.id ? .isSelected : [])
    }

    /// The chosen one's mark: the accent's check; the others, none — a long
    /// list of rings would be all the eye saw.
    @ViewBuilder
    private func mark(_ chosen: Bool) -> some View {
        if chosen {
            Image(systemName: "checkmark")
                .font(.body.weight(.semibold))
                .foregroundStyle(Color.kouAccent)
                .transition(.scale.combined(with: .opacity))
        }
    }

    private func detail(_ model: ModelInfo) -> String {
        var parts: [String] = []
        if let name = model.name, !name.isEmpty, name != model.id { parts.append(model.id) }
        if let context = Format.context(model.context) { parts.append("\(context) context") }
        if model.cooling == true { parts.append("cooling down") }
        if let via = model.via, !via.isEmpty { parts.append(via.count == 1 ? "via \(via[0])" : "via \(via.count) accounts") }
        if model.media == true { parts.append("media") }
        return parts.joined(separator: " · ")
    }

    /// The providers, as the list orders them, with their models.
    private var groups: [Group] {
        var order: [String] = []
        var byName: [String: Group] = [:]
        for model in catalog?.models ?? [] {
            let name = model.providerName.flatMap { $0.isEmpty ? nil : $0 } ?? "Other"
            if byName[name] == nil {
                order.append(name)
                byName[name] = Group(name: name, provider: model.provider)
            }
            if model.media == true {
                byName[name]?.media.append(model)
            } else {
                byName[name]?.models.append(model)
            }
        }
        return order.compactMap { byName[$0] }
    }

    private var words: [String] {
        query.lowercased().split(whereSeparator: \.isWhitespace).map(String.init)
    }

    /// What is typed, when it could be a model's ID.
    private var typed: String? {
        let text = query.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty, !text.contains(where: \.isWhitespace) else { return nil }
        return text
    }

    private func matches(_ model: ModelInfo) -> Bool {
        guard !words.isEmpty else { return true }
        let hay = ([model.id, model.name ?? "", model.providerName ?? "", model.provider ?? ""] + (model.via ?? []))
            .joined(separator: " ")
            .lowercased()
        return words.allSatisfy { hay.contains($0) }
    }

    private var footer: String {
        var text: String
        switch catalog?.source {
        case "gateway": text = "Through the accounts gateway: every model of every account."
        case "endpoint": text = "The models the connection's endpoint lists."
        default: text = "The connection does not list its models: type one's ID to use it."
        }
        text += " The choice holds for this session's next prompts; a running agent keeps its own model."
        if let error = catalog?.error, !error.isEmpty { text += "\n\n\(error)" }
        return text
    }

    private func reload(fresh: Bool) async {
        loading = true
        error = await app.loadCatalog(workspace, fresh: fresh)
        loading = false
    }

    private func pick(_ id: String?) {
        choose(id)
        dismiss()
    }
}
