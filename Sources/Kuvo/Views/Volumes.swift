import SwiftUI

// MARK: - List

/// Volumes in two groups: in use, and unused, so leftovers from removed
/// containers and old Compose projects stand out and can go in one step.
struct VolumeList: View {
    @Environment(DockerStore.self) private var store
    @Binding var selection: String?
    let query: String
    @State private var confirmRemove: [Volume] = []

    private var shown: [Volume] {
        (store.volumes ?? []).filter { volume in
            query.isEmpty || volume.name.localizedCaseInsensitiveContains(query)
                || volume.project?.localizedCaseInsensitiveContains(query) == true
        }
    }

    var body: some View {
        let shown = shown
        let unused = shown.filter { store.containers(using: $0).isEmpty }
        let used = shown.filter { !store.containers(using: $0).isEmpty }
        List(selection: $selection) {
            if !used.isEmpty {
                Section("In Use") {
                    ForEach(used) { VolumeRow(volume: $0, onRemove: nil).tag($0.name) }
                }
            }
            if !unused.isEmpty {
                Section {
                    ForEach(unused) { volume in
                        VolumeRow(volume: volume) { confirmRemove = [volume] }.tag(volume.name)
                    }
                } header: {
                    UnusedHeader(volumes: unused) { confirmRemove = unused }
                }
            }
        }
        .sidebarList()
        .overlay {
            if store.volumes == nil {
                ProgressView().controlSize(.small)
            } else if shown.isEmpty, !query.isEmpty {
                Text("No matches")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }
        }
        .confirmRemove(volumes: $confirmRemove)
        .onChange(of: store.volumes?.map(\.name)) { _, names in
            guard let names else { return }
            if selection == nil || !names.contains(selection!) {
                withAnimation(.snappy) { selection = names.first }
            }
        }
    }
}

/// "Unused · 3 · 1.2 GB" with a button to remove them all.
private struct UnusedHeader: View {
    let volumes: [Volume]
    let removeAll: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            Text("Unused")
            Text("\(volumes.count) · \(Format.bytes(Double(volumes.compactMap(\.size).reduce(0, +))))")
                .foregroundStyle(.tertiary)
                .monospacedDigit()
            Spacer(minLength: 4)
            Button("Remove All…", action: removeAll)
                .buttonStyle(.borderless)
                .font(.system(size: 11, weight: .medium))
                .help("Remove every unused volume listed here")
        }
        .padding(.trailing, 12)
        // Sidebar section headers hide their contents from accessibility; offer the button as an action.
        .accessibilityElement(children: .combine)
        .accessibilityAction(named: "Remove All Unused Volumes", removeAll)
    }
}

private struct VolumeRow: View {
    @Environment(DockerStore.self) private var store
    let volume: Volume
    /// Offered for unused volumes only; Docker won't remove one a container uses.
    let onRemove: (() -> Void)?

    private var users: [Container] { store.containers(using: volume) }
    private var removing: Bool { store.removingVolumes.contains(volume.name) }

    var body: some View {
        RowLayout(title: volume.displayName, subtitle: subtitle, action: action, trailingWidth: 56) { _ in
            Image(systemName: "cylinder")
                .font(.system(size: 12))
                .foregroundStyle(users.isEmpty ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
                .help(users.isEmpty ? "Unused" : "In use")
        } trailing: {
            Text(volume.size.map { Format.bytes(Double($0)) } ?? "—")
                .font(.system(size: 10.5, weight: .medium).monospacedDigit())
                .foregroundStyle(.secondary)
                .contentTransition(.numericText())
        }
        .opacity(removing ? 0.5 : 1)
        .contextMenu {
            Button("Copy Name") { copy(volume.name) }
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
        if !users.isEmpty { return "Used by " + users.map { $0.service ?? $0.name }.joined(separator: ", ") }
        // A project keeps its own spelling, so only "Created" is capitalized on its own.
        let origin = volume.project ?? (volume.isAnonymous ? "Anonymous" : nil)
        guard let created = volume.createdAt?.formatted(.relative(presentation: .named)) else { return origin ?? "Unused" }
        return origin.map { "\($0) · created \(created)" } ?? "Created \(created)"
    }
}

// MARK: - Detail

/// The volumes section's detail column: the selected volume, or a hint.
struct VolumesDetail: View {
    @Environment(DockerStore.self) private var store
    @Binding var selection: String?
    let openContainer: (Container) -> Void
    @State private var creating = false

    private var selected: Volume? { store.volumes?.first { $0.name == selection } }

    var body: some View {
        ZStack {
            if let selected {
                VolumeDetail(volume: selected, openContainer: openContainer)
                    .id(selected.name)
                    .transition(.opacity.combined(with: .offset(y: 6)))
            } else if store.volumes?.isEmpty == true {
                ContentUnavailableView {
                    Label("No Volumes", systemImage: "cylinder")
                } description: {
                    Text("Volumes keep data, like a database's files, after its container is removed.")
                } actions: {
                    Button("New Volume…") { creating = true }
                }
                .transition(.opacity)
                .navigationTitle("Volumes")
            } else {
                Color.clear.navigationTitle("Volumes")
            }
        }
        .animation(.snappy(duration: 0.28), value: selection)
        .toolbar {
            ToolbarItem {
                Button { creating = true } label: { Label("New Volume", systemImage: "plus") }
                    .help("New volume (⌘N)")
                    .keyboardShortcut("n", modifiers: .command)
            }
        }
        .sheet(isPresented: $creating) {
            NewVolumeSheet { name in withAnimation(.snappy) { selection = name } }
        }
    }
}

private struct VolumeDetail: View {
    @Environment(DockerStore.self) private var store
    let volume: Volume
    let openContainer: (Container) -> Void
    @State private var confirmRemove: [Volume] = []

    private var users: [Container] { store.containers(using: volume) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                usedBy
                details
            }
            .padding([.horizontal, .bottom], 20)
            .padding(.top, 8)
        }
        .navigationTitle(volume.displayName)
        .navigationSubtitle(subtitle)
        .toolbar {
            ToolbarItem(placement: .navigation) { TitleIcon(systemName: "cylinder.fill") }
            ToolbarItem {
                Button { confirmRemove = [volume] } label: { Label("Remove", systemImage: "trash") }
                    .help(users.isEmpty ? "Remove" : "In use: remove the containers using it first")
                    .disabled(!users.isEmpty || store.removingVolumes.contains(volume.name))
            }
        }
        .confirmRemove(volumes: $confirmRemove)
    }

    private var subtitle: String {
        let size = volume.size.map { Format.bytes(Double($0)) }
        let age = volume.createdAt.map { "created \($0.formatted(.relative(presentation: .named)))" }
        return [size, age].compactMap(\.self).joined(separator: "  ·  ").capitalizedSentence
    }

    private var usedBy: some View {
        Pane(icon: "shippingbox", title: "Used By") {
            EmptyView()
        } content: {
            if users.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("No container uses this volume.")
                        .font(.system(size: 13))
                    Text("Removing it deletes the data in it.")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 14)
                .padding(.bottom, 14)
            } else {
                VStack(spacing: 0) {
                    ForEach(users) { container in
                        UserRow(container: container) { openContainer(container) }
                    }
                }
                .padding(.horizontal, 6)
                .padding(.bottom, 6)
            }
        }
    }

    private var details: some View {
        Pane(icon: "info.circle", title: "Details") {
            EmptyView()
        } content: {
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 16, verticalSpacing: 8) {
                DetailRow("Name", volume.name, monospaced: volume.isAnonymous)
                if let size = volume.size { DetailRow("Size", Format.bytes(Double(size))) }
                if let created = volume.createdAt {
                    DetailRow("Created", created.formatted(date: .abbreviated, time: .shortened))
                }
                if let project = volume.project { DetailRow("Compose project", project) }
                if volume.isAnonymous { DetailRow("Kind", "Anonymous, made by a container or image without a name") }
                DetailRow("Driver", volume.driver)
                DetailRow("Path in the VM", volume.mountpoint, monospaced: true)
                ForEach(labels, id: \.key) { label in
                    DetailRow(label.key, label.value, monospaced: true)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.bottom, 14)
        }
    }

    /// Labels other than the ones already shown above.
    private var labels: [(key: String, value: String)] {
        volume.labels
            .filter { !$0.key.hasPrefix("com.docker.compose.") && $0.key != "com.docker.volume.anonymous" }
            .sorted { $0.key < $1.key }
    }
}

private struct DetailRow: View {
    let label: String
    let value: String
    var monospaced = false

    init(_ label: String, _ value: String, monospaced: Bool = false) {
        self.label = label
        self.value = value
        self.monospaced = monospaced
    }

    var body: some View {
        GridRow {
            Text(label)
                .foregroundStyle(.secondary)
                .gridColumnAlignment(.trailing)
            Text(value)
                .font(monospaced ? .system(size: 12).monospaced() : .system(size: 13))
                .textSelection(.enabled)
                .lineLimit(2)
                .truncationMode(.middle)
        }
        .font(.system(size: 13))
    }
}

/// A container using the volume; clicking it opens the container.
private struct UserRow: View {
    let container: Container
    let open: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: open) {
            HStack(spacing: 10) {
                StatusDot(state: container.state)
                VStack(alignment: .leading, spacing: 1) {
                    Text(container.name).font(.system(size: 13, weight: .medium))
                    Text("\(container.humanStatus) · \(container.image)")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                .lineLimit(1)
                Spacer(minLength: 8)
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.tertiary)
                    .opacity(hovering ? 1 : 0)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(.primary.opacity(hovering ? 0.06 : 0), in: .rect(cornerRadius: 8))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .help("Show \(container.name)")
    }
}

// MARK: - Create

private struct NewVolumeSheet: View {
    @Environment(DockerStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    let onCreated: (String) -> Void
    @State private var name = ""
    @State private var error: String?
    @State private var creating = false

    /// Docker's rule for volume names.
    private var isValid: Bool { name.wholeMatch(of: /[a-zA-Z0-9][a-zA-Z0-9_.-]+/) != nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New Volume").font(.headline)
            TextField("Name", text: $name, prompt: Text("e.g. postgres-data"))
                .textFieldStyle(.roundedBorder)
                .onSubmit(create)
            Text(error ?? "Letters, numbers, and . _ - after the first character.")
                .font(.system(size: 11))
                .foregroundStyle(error == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(.red))
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Create", action: create)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!isValid || creating)
            }
        }
        .padding(20)
        .frame(width: 340)
        .onChange(of: name) { error = nil }
    }

    private func create() {
        guard isValid, !creating else { return }
        // Docker returns the existing volume instead of failing, so check first.
        guard store.volumes?.contains(where: { $0.name == name }) != true else {
            error = "A volume named \(name) already exists."
            return
        }
        creating = true
        Task {
            do {
                try await store.createVolume(named: name)
                onCreated(name)
                dismiss()
            } catch {
                self.error = error.localizedDescription
                creating = false
            }
        }
    }
}

// MARK: - Remove

extension View {
    /// Asks before removing volumes; shown while `volumes` isn't empty.
    func confirmRemove(volumes: Binding<[Volume]>) -> some View {
        modifier(RemoveVolumesConfirmation(volumes: volumes))
    }
}

private struct RemoveVolumesConfirmation: ViewModifier {
    @Environment(DockerStore.self) private var store
    @Binding var volumes: [Volume]

    func body(content: Content) -> some View {
        content.confirmationDialog(
            title,
            isPresented: Binding(get: { !volumes.isEmpty }, set: { if !$0 { volumes = [] } }),
            presenting: volumes
        ) { targets in
            Button(targets.count == 1 ? "Remove" : "Remove \(targets.count) Volumes", role: .destructive) {
                store.removeVolumes(targets.map(\.name))
            }
        } message: { targets in
            Text(message(targets))
        }
    }

    private var title: String {
        volumes.count == 1 ? "Remove \(volumes[0].displayName)?" : "Remove \(volumes.count) unused volumes?"
    }

    /// Names what goes, so nothing is removed by surprise.
    private func message(_ targets: [Volume]) -> String {
        let size = targets.compactMap(\.size).reduce(0, +)
        let amount = size > 0 ? "the \(Format.bytes(Double(size))) of data" : "the data"
        if targets.count == 1 { return "\(amount.capitalizedSentence) in it is deleted. This can't be undone." }
        let names = targets.count <= 6 ? targets.map(\.displayName).formatted(.list(type: .and)) : "All \(targets.count)"
        return "\(names) are deleted, with \(amount) in them. This can't be undone."
    }
}

private func copy(_ text: String) {
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(text, forType: .string)
}
