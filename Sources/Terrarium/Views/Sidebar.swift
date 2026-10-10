import SwiftUI

/// One list: Compose projects are collapsible groups, everything else is a plain row.
struct Sidebar: View {
    @Environment(DockerStore.self) private var store
    @Binding var selection: String?
    @State private var query = ""
    @State private var collapsed: Set<String> = []

    private enum Item: Identifiable {
        case project(String, [Container])
        case container(Container)

        var id: String {
            switch self {
            case .project(let name, _): "project:\(name)"
            case .container(let container): container.id
            }
        }
    }

    private var items: [Item] {
        let matches = store.containers.filter {
            query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) || $0.image.localizedCaseInsensitiveContains(query)
        }
        var items: [Item] = []
        var seen = Set<String>()
        for container in matches {
            guard let project = container.project else {
                items.append(.container(container))
                continue
            }
            if seen.insert(project).inserted {
                items.append(.project(project, matches.filter { $0.project == project }))
            }
        }
        return items
    }

    var body: some View {
        List(selection: $selection) {
            ForEach(items) { item in
                switch item {
                case .project(let name, let containers):
                    DisclosureGroup(isExpanded: expansion(name)) {
                        ForEach(containers) { row($0) }
                    } label: {
                        ProjectRow(project: name, containers: containers)
                    }
                case .container(let container):
                    row(container)
                }
            }
        }
        .listStyle(.sidebar)
        .searchable(text: $query, placement: .sidebar, prompt: "Filter")
        .safeAreaInset(edge: .bottom, spacing: 0) { Footer() }
    }

    private func row(_ container: Container) -> some View {
        ContainerRow(container: container)
            .tag(container.id)
            .transition(.asymmetric(insertion: .push(from: .leading), removal: .opacity.combined(with: .scale(scale: 0.9))))
    }

    /// Searching expands everything so matches are never hidden.
    private func expansion(_ project: String) -> Binding<Bool> {
        Binding(
            get: { !query.isEmpty || !collapsed.contains(project) },
            set: { expanded in withAnimation(.snappy) { if expanded { collapsed.remove(project) } else { collapsed.insert(project) } } }
        )
    }
}

// MARK: - Rows

/// Shared layout for every sidebar row: a 16pt leading glyph, two lines of
/// text, and a fixed trailing slot that swaps to an action on hover without
/// changing the row's size.
private struct RowLayout<Glyph: View, Trailing: View>: View {
    let title: String
    let subtitle: String
    let action: RowAction?
    @ViewBuilder var glyph: Glyph
    @ViewBuilder var trailing: Trailing
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 8) {
            glyph.frame(width: 16)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(.system(size: 13, weight: .medium))
                Text(subtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .contentTransition(.interpolate)
            }
            .lineLimit(1)
            Spacer(minLength: 4)
            ZStack(alignment: .trailing) {
                if hovering, let action {
                    action.transition(.opacity)
                } else {
                    trailing.transition(.opacity)
                }
            }
            .frame(width: 84, height: 24, alignment: .trailing)
            .animation(.easeOut(duration: 0.12), value: hovering)
        }
        .padding(.vertical, 3)
        .contentShape(.rect)
        .onHover { hovering = $0 }
    }
}

/// A real, comfortably sized button for row actions (not a bare glyph).
private struct RowAction: View {
    let systemImage: String
    let help: String
    let perform: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: perform) {
            Image(systemName: systemImage)
                .font(.system(size: 10, weight: .semibold))
                .frame(width: 28, height: 22)
                .background(.primary.opacity(hovering ? 0.16 : 0.08), in: .rect(cornerRadius: 6))
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(help)
    }
}

private struct ProjectRow: View {
    @Environment(DockerStore.self) private var store
    let project: String
    let containers: [Container]

    private var running: Int { containers.filter(\.isRunning).count }

    var body: some View {
        RowLayout(title: project, subtitle: subtitle, action: action) {
            Image(systemName: "square.stack.3d.up")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
        } trailing: {
            ProjectCPU(stats: containers.filter(\.isRunning).map { store.stats(for: $0.id) })
        }
        .contextMenu {
            Button("Start All") { store.perform(.start, onProject: project) }.disabled(running == containers.count)
            Button("Stop All") { store.perform(.stop, onProject: project) }.disabled(running == 0)
        }
    }

    private var subtitle: String {
        switch running {
        case 0: "Stopped"
        case containers.count: containers.count == 1 ? "Running" : "All \(containers.count) running"
        default: "\(running) of \(containers.count) running"
        }
    }

    private var action: RowAction {
        running > 0
            ? RowAction(systemImage: "stop.fill", help: "Stop all in \(project)") { store.perform(.stop, onProject: project) }
            : RowAction(systemImage: "play.fill", help: "Start all in \(project)") { store.perform(.start, onProject: project) }
    }
}

/// Combined CPU for a project, in the same column as each container's stats.
private struct ProjectCPU: View {
    let stats: [LiveStats]

    var body: some View {
        let total = stats.reduce(0) { $0 + ($1.cpu.last ?? 0) }
        Text(stats.isEmpty ? "" : Format.percent(total))
            .font(.system(size: 10.5, weight: .medium).monospacedDigit())
            .foregroundStyle(.secondary)
            .contentTransition(.numericText(value: total))
            .animation(.snappy, value: total)
    }
}

private struct ContainerRow: View {
    @Environment(DockerStore.self) private var store
    let container: Container

    private var busy: ContainerAction? { store.busy[container.id] }

    var body: some View {
        RowLayout(title: container.service ?? container.name, subtitle: subtitle, action: action) {
            StatusDot(state: container.state, busy: busy != nil)
        } trailing: {
            if container.isRunning {
                RowStats(stats: store.stats(for: container.id))
            }
        }
        .contextMenu { ContainerMenu(container: container) }
    }

    private var action: RowAction? {
        guard busy == nil else { return nil }
        return container.isRunning
            ? RowAction(systemImage: "stop.fill", help: "Stop") { store.perform(.stop, on: container) }
            : RowAction(systemImage: "play.fill", help: "Start") { store.perform(.start, on: container) }
    }

    private var subtitle: String {
        if let busy { return "\(busy.progressLabel)…" }
        let ports = container.publishedPorts.prefix(2).map { ":\($0)" }.joined(separator: " ")
        return ports.isEmpty ? container.image : "\(container.image) · \(ports)"
    }
}

/// Isolated so a stats tick only re-renders this tiny view.
private struct RowStats: View {
    let stats: LiveStats

    var body: some View {
        HStack(spacing: 6) {
            Sparkline(values: stats.cpu, tick: stats.tick, minimum: 5, lineWidth: 1.25)
                .frame(width: 40, height: 16)
            Text(Format.percent(stats.cpu.last ?? 0))
                .font(.system(size: 10.5, weight: .medium).monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 34, alignment: .trailing)
                .contentTransition(.numericText(value: stats.cpu.last ?? 0))
                .animation(.snappy, value: stats.tick)
        }
        .opacity(stats.hasData ? 1 : 0)
        .animation(.easeOut(duration: 0.4), value: stats.hasData)
    }
}

private struct Footer: View {
    @Environment(DockerStore.self) private var store

    private var running: Int { store.containers.filter(\.isRunning).count }

    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(.green).frame(width: 6, height: 6)
            if case .connected(let engine) = store.connection {
                Text(engine).lineLimit(1)
            }
            Spacer()
            Text("\(running) running")
                .monospacedDigit()
                .contentTransition(.numericText(value: Double(running)))
                .animation(.snappy, value: running)
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }
}

struct ContainerMenu: View {
    @Environment(DockerStore.self) private var store
    let container: Container

    var body: some View {
        if container.isRunning {
            Button("Stop") { store.perform(.stop, on: container) }
            Button("Restart") { store.perform(.restart, on: container) }
            Button("Pause") { store.perform(.pause, on: container) }
        } else if container.state == .paused {
            Button("Resume") { store.perform(.unpause, on: container) }
        } else {
            Button("Start") { store.perform(.start, on: container) }
        }
        if let port = container.publishedPorts.first, container.isRunning {
            Divider()
            Button("Open localhost:\(String(port))") { NSWorkspace.shared.open(URL(string: "http://localhost:\(port)")!) }
        }
        Divider()
        Button("Copy ID") { copy(container.id) }
        Button("Copy Name") { copy(container.name) }
        Divider()
        Button("Remove", role: .destructive) { store.perform(.remove, on: container) }
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}
