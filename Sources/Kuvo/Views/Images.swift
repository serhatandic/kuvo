import SwiftUI

extension DockerImage {
    /// The name to show: the tag a container was run with, else a specific tag
    /// over "latest", else the first. An untagged image goes by its short ID, so it
    /// isn't mistaken for the image that took its tag; what it was is in the subtitle.
    func title(users: [Container]) -> String {
        if isUntagged { return shortID }
        if let name = users.lazy.map(\.image).first(where: { tags.contains($0) }) { return name }
        return tags.first { !$0.hasSuffix(":latest") } ?? tags[0]
    }
}

// MARK: - List

/// One row per image, however many tags it has, so sizes aren't counted twice.
/// In use and unused, like volumes; untagged is something a row says, not a group.
struct ImageList: View {
    @Environment(DockerStore.self) private var store
    @Binding var selection: String?
    let query: String
    @State private var confirmRemove: DockerImage?
    @State private var confirmCleanup: [DockerImage] = []

    private var shown: [DockerImage] {
        (store.images ?? []).filter { image in
            query.isEmpty || image.tags.contains { $0.localizedCaseInsensitiveContains(query) }
                || image.repository?.localizedCaseInsensitiveContains(query) == true
                || image.shortID.hasPrefix(query.lowercased())
        }
    }

    var body: some View {
        let shown = shown
        let used = shown.filter { !store.containers(using: $0).isEmpty }
        let unused = shown.filter { store.containers(using: $0).isEmpty }
        List(selection: $selection) {
            if !store.pulls.isEmpty {
                Section {
                    ForEach(store.pulls) { PullRow(pull: $0).selectionDisabled() }
                }
            }
            if !used.isEmpty {
                Section("In Use") {
                    ForEach(used) { ImageRow(image: $0, onRemove: nil).tag($0.id) }
                }
            }
            if !unused.isEmpty {
                Section {
                    ForEach(unused) { image in
                        ImageRow(image: image) { confirmRemove = image }.tag(image.id)
                    }
                } header: {
                    CleanupHeader(count: unused.count, size: unused.reduce(0) { $0 + $1.size }, noun: "images") { confirmCleanup = unused }
                }
            }
        }
        .sidebarList()
        .overlay {
            if store.images == nil {
                ProgressView().controlSize(.small)
            } else if shown.isEmpty, !query.isEmpty {
                Text("No matches")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }
        }
        .confirmRemove(image: $confirmRemove)
        .confirmCleanup(images: $confirmCleanup)
        .onChange(of: store.images?.map(\.id), initial: true) { _, ids in
            guard let ids else { return }
            if selection == nil || !ids.contains(selection!) {
                withAnimation(.snappy) { selection = ids.first }
            }
        }
        .onChange(of: store.pulledImageID) { _, id in
            if let id { withAnimation(.snappy) { selection = id } }
        }
    }
}

private struct ImageRow: View {
    @Environment(DockerStore.self) private var store
    let image: DockerImage
    /// Offered for unused images only; Docker won't remove one a container uses.
    let onRemove: (() -> Void)?

    private var users: [Container] { store.containers(using: image) }
    private var removing: Bool { store.removingImages.contains(image.id) }

    var body: some View {
        RowLayout(title: image.title(users: users), subtitle: subtitle, action: action, trailingWidth: 56) { _ in
            Image(systemName: "square.3.layers.3d")
                .font(.system(size: 12))
                .foregroundStyle(users.isEmpty ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
                .help(users.isEmpty ? "Unused" : "In use")
        } trailing: {
            Text(Format.bytes(Double(image.size)))
                .font(.system(size: 10.5, weight: .medium).monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .opacity(removing ? 0.5 : 1)
        .contextMenu {
            Button("Copy Name") { copyToPasteboard(image.title(users: users)) }
            Button("Copy ID") { copyToPasteboard(image.id) }
            if let onRemove {
                Divider()
                Button("Remove…", role: .destructive, action: onRemove)
            }
        }
    }

    private var action: RowAction? {
        guard let onRemove, !removing else { return nil }
        return RowAction(systemImage: "trash", help: "Remove", perform: onRemove)
    }

    private var subtitle: String {
        if removing { return "Removing…" }
        let origin = image.isUntagged ? store.formerTags(of: image).first.map { "was \($0)" } ?? "untagged" : nil
        if !users.isEmpty {
            let names = "used by " + users.map { $0.service ?? $0.name }.joined(separator: ", ")
            return [origin, names].compactMap(\.self).joined(separator: " · ").capitalizedSentence
        }
        let extraTags = max(image.tags.count - 1, 0)
        let parts = [
            origin,
            "built \(image.created.formatted(.relative(presentation: .named)))",
            extraTags > 0 ? "+\(extraTags) \(extraTags == 1 ? "tag" : "tags")" : nil,
        ]
        return parts.compactMap(\.self).joined(separator: " · ").capitalizedSentence
    }
}

/// A pull in progress, above the images. Docker announces each layer's size as it
/// starts on it, so this shows bytes rather than a percentage that would jump back.
private struct PullRow: View {
    let pull: ImagePull

    var body: some View {
        RowLayout(title: pull.name, subtitle: subtitle, action: nil, trailingWidth: 56) { _ in
            Image(systemName: pull.upToDate ? "checkmark.circle" : "arrow.down.circle")
                .font(.system(size: 12))
                .foregroundStyle(pull.upToDate ? AnyShapeStyle(.green) : AnyShapeStyle(.tint))
                .contentTransition(.symbolEffect(.replace))
        } trailing: {
            if !pull.upToDate {
                ProgressView(value: pull.progress.fraction ?? 0)
                    .progressViewStyle(.linear)
                    .controlSize(.small)
                    .frame(width: 40)
            }
        }
    }

    private var subtitle: String {
        if pull.upToDate { return "Already up to date" }
        let progress = pull.progress
        guard progress.size > 0 else { return "Pulling…" }
        return "Pulling · \(Format.bytes(Double(progress.downloaded))) of \(Format.bytes(Double(progress.size)))"
    }
}

// MARK: - Detail

/// The images section's detail column: the selected image, or a hint.
struct ImagesDetail: View {
    @Environment(DockerStore.self) private var store
    @Binding var selection: String?
    let openContainer: (Container) -> Void
    @State private var pulling = false

    private var selected: DockerImage? { store.images?.first { $0.id == selection } }

    var body: some View {
        ZStack {
            if let selected {
                ImageDetail(image: selected, openContainer: openContainer)
                    .id(selected.id)
                    .transition(.opacity.combined(with: .offset(y: 6)))
            } else if store.images?.isEmpty == true, store.pulls.isEmpty {
                ContentUnavailableView {
                    Label("No Images", systemImage: "square.3.layers.3d")
                } description: {
                    Text("Images show up here when you run a container, build an image or pull one.")
                } actions: {
                    Button("Pull Image…") { pulling = true }
                }
                .transition(.opacity)
                .navigationTitle("Images")
            } else {
                Color.clear.navigationTitle("Images")
            }
        }
        .animation(.snappy(duration: 0.28), value: selection)
        .toolbar {
            ToolbarItem {
                Button { pulling = true } label: { Label("Pull Image", systemImage: "arrow.down.circle") }
                    .help("Pull an image (⌘N)")
                    .keyboardShortcut("n", modifiers: .command)
            }
        }
        .sheet(isPresented: $pulling) { PullSheet() }
    }
}

/// Answers, in order: is anything using this image, and if not, why is it still
/// here (the subtitle); which containers run it (Containers); and what it is
/// (Overview: tags, size, age, platform, ports, command).
private struct ImageDetail: View {
    @Environment(DockerStore.self) private var store
    let image: DockerImage
    let openContainer: (Container) -> Void
    @State private var details: ImageDetails?
    @State private var confirmRemove: DockerImage?

    private var users: [Container] { store.containers(using: image) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                if !users.isEmpty { containers }
                overview
            }
            .padding([.horizontal, .bottom], 20)
            .padding(.top, 8)
        }
        .navigationTitle(image.title(users: users))
        .navigationSubtitle(verdict)
        .toolbar {
            ToolbarItem(placement: .navigation) { TitleIcon(systemName: "square.3.layers.3d") }
            ToolbarItem {
                Button { confirmRemove = image } label: { Label("Remove", systemImage: "trash") }
                    .help(users.isEmpty ? "Remove" : removeBlockedHelp(users))
                    .disabled(!users.isEmpty || store.removingImages.contains(image.id))
            }
        }
        .task(id: image.id) { details = try? await store.client?.inspectImage(image.id) }
        .confirmRemove(image: $confirmRemove)
    }

    /// Who uses it, or why an unused one might still be here. An image goes untagged
    /// when its tag moves to another image, usually a newer version that was pulled.
    private var verdict: String {
        let former = store.formerTags(of: image).first
        let moved = former.map { tag in store.images?.contains { $0.tags.contains(tag) } == true } ?? false
        if !users.isEmpty {
            let used = usedBy(users)
            guard image.isUntagged else { return used }
            let lowered = used.prefix(1).lowercased() + used.dropFirst()
            // Its tag moved on, but these containers still run this one.
            return former.map { "Older version of \($0)  ·  \(lowered)" } ?? "Untagged, older version  ·  \(lowered)"
        }
        guard image.isUntagged else { return "Not in use" }
        guard let former else { return "Not in use  ·  untagged" }
        return moved ? "Not in use  ·  \(former) now points to a newer image" : "Not in use  ·  was \(former)"
    }

    private var containers: some View {
        Pane(icon: "shippingbox", title: "Containers") {
            EmptyView()
        } content: {
            VStack(spacing: 0) {
                ForEach(users) { container in
                    ContainerLinkRow(container: container) { openContainer(container) }
                }
            }
            .padding(.horizontal, 6)
            .padding(.bottom, 6)
        }
    }

    private var overview: some View {
        Pane(icon: "info.circle", title: "Overview") {
            EmptyView()
        } content: {
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 16, verticalSpacing: 8) {
                if image.isUntagged {
                    let former = store.formerTags(of: image)
                    DetailRow(former.isEmpty ? "Tags" : "Was tagged", former.isEmpty ? "None" : former.joined(separator: "\n"))
                } else {
                    TagsRow(tags: image.tags)
                }
                DetailRow("Size", Format.bytes(Double(image.size)))
                DetailRow("Built", image.created.formatted(.relative(presentation: .named)).capitalizedSentence,
                          help: image.created.formatted(date: .long, time: .shortened))
                if let platform = details?.platform {
                    // Kuvo runs Intel images through Rosetta, which is worth knowing when one is slow.
                    DetailRow("Platform", platform.contains("amd64") ? "\(platform)  ·  runs through Rosetta" : platform)
                }
                if let ports = details?.exposedPorts, !ports.isEmpty {
                    DetailRow("Exposes", ports.map { $0.hasSuffix("/tcp") ? String($0.dropLast(4)) : $0 }.joined(separator: ", "))
                }
                if let command = details?.command { DetailRow("Command", command, monospaced: true) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.bottom, 14)
        }
    }
}

/// Every tag on its own line; right-click one to copy it, or to remove it while others remain.
private struct TagsRow: View {
    @Environment(DockerStore.self) private var store
    let tags: [String]

    var body: some View {
        GridRow {
            Text(tags.count == 1 ? "Tag" : "Tags")
                .foregroundStyle(.secondary)
                .gridColumnAlignment(.trailing)
            VStack(alignment: .leading, spacing: 3) {
                ForEach(tags, id: \.self) { tag in
                    Text(tag)
                        .textSelection(.enabled)
                        .contextMenu {
                            Button("Copy Tag") { copyToPasteboard(tag) }
                            if tags.count > 1 {
                                Button("Remove Tag", role: .destructive) { store.removeTag(tag) }
                            }
                        }
                }
            }
        }
        .font(.system(size: 13))
    }
}

// MARK: - Pull

private struct PullSheet: View {
    @Environment(DockerStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var error: String?
    @State private var starting = false

    private var reference: String { name.trimmingCharacters(in: .whitespaces) }

    /// Docker's image reference format, loosely: an optional registry, lowercase
    /// path components, then an optional tag and digest. Docker checks the rest.
    private var isValid: Bool {
        reference.wholeMatch(of: /(?:[a-zA-Z0-9.-]+(?::[0-9]+)?\/)?[a-z0-9]+(?:(?:[._]|__|-+)[a-z0-9]+)*(?:\/[a-z0-9]+(?:(?:[._]|__|-+)[a-z0-9]+)*)*(?::\w[\w.-]{0,127})?(?:@sha256:[a-f0-9]{64})?/) != nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Pull Image").font(.headline)
            TextField("Image", text: $name, prompt: Text("nginx:alpine"))
                .textFieldStyle(.roundedBorder)
                .onSubmit(pull)
            Text(message)
                .font(.system(size: 11))
                .foregroundStyle(error == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(.red))
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                if starting {
                    ProgressView().controlSize(.small)
                    Text("Connecting…").font(.system(size: 12)).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Pull", action: pull)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!isValid || starting)
            }
        }
        .padding(20)
        .frame(width: 360)
        .onChange(of: name) { error = nil }
    }

    private var message: String {
        if let error { return error }
        if !reference.isEmpty, !isValid { return "That isn't a valid image name. Names are lowercase, like nginx:alpine." }
        return "Leave out the tag to pull latest. Pulling an image you have gets its newest version."
    }

    private func pull() {
        guard isValid, !starting else { return }
        starting = true
        let reference = reference
        Task {
            do {
                try await store.startPull(reference)
                dismiss()
            } catch {
                self.error = Self.explain(error, reference: reference)
                starting = false
            }
        }
    }

    /// Docker's messages, in plain words for the cases people hit.
    private static func explain(_ error: Error, reference: String) -> String {
        let message = error.localizedDescription
        let lower = message.lowercased()
        if lower.contains("pull access denied") || lower.contains("unauthorized") || lower.contains("docker login") {
            return "\(reference) wasn't found, or it needs a login. Check the name, or run docker login in a terminal first."
        }
        if lower.contains("not found") || lower.contains("manifest unknown") {
            return "\(reference) wasn't found. Check the name and tag."
        }
        if lower.contains("invalid reference format") { return "That isn't a valid image name." }
        return message
    }
}

// MARK: - Remove

extension View {
    /// Asks before removing one image, naming its other tags, which go with it.
    func confirmRemove(image: Binding<DockerImage?>) -> some View {
        modifier(RemoveImageConfirmation(image: image))
    }

    /// Asks before removing unused images, offering just the untagged ones when there are both.
    func confirmCleanup(images: Binding<[DockerImage]>) -> some View {
        modifier(CleanupImagesConfirmation(images: images))
    }
}

private struct RemoveImageConfirmation: ViewModifier {
    @Environment(DockerStore.self) private var store
    @Binding var image: DockerImage?

    func body(content: Content) -> some View {
        content.confirmationDialog(
            image.map { "Remove \($0.title(users: []))?" } ?? "",
            isPresented: Binding(get: { image != nil }, set: { if !$0 { image = nil } }),
            presenting: image
        ) { target in
            Button("Remove", role: .destructive) { store.removeImages([target]) }
        } message: { target in
            Text(message(target))
        }
    }

    private func message(_ image: DockerImage) -> String {
        let title = image.title(users: [])
        let others = image.tags.filter { $0 != title }
        let also = others.isEmpty ? "" : "It's also tagged \(others.formatted(.list(type: .and))); \(others.count == 1 ? "both go" : "all go"). "
        let size = "This frees up to \(Format.bytes(Double(image.size)))."
        return also + size + (image.isUntagged ? "" : " Pulling it again downloads it.")
    }
}

private struct CleanupImagesConfirmation: ViewModifier {
    @Environment(DockerStore.self) private var store
    @Binding var images: [DockerImage]

    func body(content: Content) -> some View {
        content.confirmationDialog(
            "Remove \(images.count) unused \(images.count == 1 ? "image" : "images")?",
            isPresented: Binding(get: { !images.isEmpty }, set: { if !$0 { images = [] } }),
            presenting: images
        ) { targets in
            let untagged = targets.filter(\.isUntagged)
            if !untagged.isEmpty, untagged.count < targets.count {
                Button("Remove \(untagged.count) Untagged (\(size(untagged)))", role: .destructive) { store.removeImages(untagged) }
            }
            Button(targets.count == 1 ? "Remove (\(size(targets)))" : "Remove All \(targets.count) (\(size(targets)))", role: .destructive) {
                store.removeImages(targets)
            }
        } message: { targets in
            let names = targets.count <= 6 ? targets.map { $0.title(users: []) }.formatted(.list(type: .and)) + ". " : ""
            Text("\(names)No container uses them. Images can share layers, so less space may be freed than their sizes add up to. Pulling an image again downloads it.")
        }
    }

    private func size(_ images: [DockerImage]) -> String {
        Format.bytes(Double(images.reduce(0) { $0 + $1.size }))
    }
}
