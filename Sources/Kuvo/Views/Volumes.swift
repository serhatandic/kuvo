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
                    CleanupHeader(count: unused.count, size: unused.compactMap(\.size).reduce(0, +), noun: "volumes") { confirmRemove = unused }
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
            Button("Copy Name") { copyToPasteboard(volume.name) }
            Button("Copy Path in the VM") { copyToPasteboard(volume.mountpoint) }
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
                    Text("Volumes show up here when a container stores data in one, like a database's files. They keep it after the container is removed.")
                }
                .transition(.opacity)
                .navigationTitle("Volumes")
            } else {
                Color.clear.navigationTitle("Volumes")
            }
        }
        .animation(.snappy(duration: 0.28), value: selection)
    }
}

/// Answers, in order: is anything using this volume, and if not, why is it still
/// here (the subtitle); where each container keeps its data in it (Mounts); and
/// how big and old it is (Overview). Technical facts nobody reads twice, like
/// its path inside the VM, are copy actions in the sidebar row's menu.
private struct VolumeDetail: View {
    @Environment(DockerStore.self) private var store
    let volume: Volume
    let openContainer: (Container) -> Void
    @State private var confirmRemove: [Volume] = []

    private var users: [Container] { store.containers(using: volume) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                if !users.isEmpty { mounts }
                // The planned file browser goes here, between Mounts and Overview.
                overview
            }
            .padding([.horizontal, .bottom], 20)
            .padding(.top, 8)
        }
        .navigationTitle(volume.displayName)
        .navigationSubtitle(VolumeStatus(volume: volume, users: users, store: store).verdict)
        .toolbar {
            ToolbarItem(placement: .navigation) { TitleIcon(systemName: "cylinder.fill") }
            ToolbarItem {
                Button { confirmRemove = [volume] } label: { Label("Remove", systemImage: "trash") }
                    .help(users.isEmpty ? "Remove" : removeBlockedHelp(users))
                    .disabled(!users.isEmpty || store.removingVolumes.contains(volume.name))
            }
        }
        .confirmRemove(volumes: $confirmRemove)
    }

    private var mounts: some View {
        Pane(icon: "shippingbox", title: "Mounts") {
            EmptyView()
        } content: {
            VStack(spacing: 0) {
                ForEach(users) { container in
                    if let mount = container.volumeMounts.first(where: { $0.name == volume.name }) {
                        ContainerLinkRow(
                            container: container,
                            accessibilityDetail: "mounted at \(mount.destination)\(mount.readOnly ? ", read-only" : "")",
                            open: { openContainer(container) }
                        ) {
                            MountPath(mount: mount, container: container)
                        }
                    }
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
                DetailRow("Size", volume.size.map { Format.bytes(Double($0)) } ?? "Not measured")
                if let created = volume.createdAt {
                    DetailRow("Created", created.formatted(.relative(presentation: .named)).capitalizedSentence,
                              help: created.formatted(date: .long, time: .shortened))
                }
                if let project = volume.project { DetailRow("Compose project", project) }
                if volume.isAnonymous { DetailRow("Origin", "Anonymous, from an image's VOLUME or a -v /path mount") }
                if volume.driver != "local" { DetailRow("Driver", volume.driver) }
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

/// The one-line answer under the title: who uses the volume, or why an unused one exists.
@MainActor private struct VolumeStatus {
    let volume: Volume
    let users: [Container]
    let store: DockerStore

    var verdict: String {
        if !users.isEmpty { return usedBy(users) }
        // Its age is in Overview, so it isn't repeated here.
        if let project = volume.project {
            return store.containers.contains(where: { $0.project == project })
                ? "Not in use  ·  from the Compose project \(project)"
                : "Not in use  ·  left by the Compose project \(project), which has no containers"
        }
        return volume.isAnonymous ? "Not in use  ·  anonymous" : "Not in use"
    }
}

/// Where the volume appears inside a container, and whether it's read-only.
private struct MountPath: View {
    let mount: Container.VolumeMount
    let container: Container

    var body: some View {
        VStack(alignment: .trailing, spacing: 1) {
            Text(mount.destination)
                .font(.system(size: 12).monospaced())
                .truncationMode(.head)
            if mount.readOnly {
                Text("read-only")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
        }
        .lineLimit(1)
        .help("Mounted at \(mount.destination) inside \(container.name)\(mount.readOnly ? ", read-only" : "")")
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
