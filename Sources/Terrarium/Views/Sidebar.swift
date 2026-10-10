import SwiftUI

struct Sidebar: View {
    @Environment(DockerStore.self) private var store
    @Binding var selection: String?
    @State private var query = ""
    @State private var collapsed: Set<String> = []

    private struct Group: Identifiable {
        let project: String?
        let containers: [Container]
        var id: String { project ?? "" }
    }

    private var groups: [Group] {
        let matches = store.containers.filter {
            query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) || $0.image.localizedCaseInsensitiveContains(query)
        }
        var order: [String?] = []
        var byProject: [String?: [Container]] = [:]
        for container in matches {
            if byProject[container.project] == nil { order.append(container.project) }
            byProject[container.project, default: []].append(container)
        }
        return order.map { Group(project: $0, containers: byProject[$0]!) }
    }

    var body: some View {
        List(selection: $selection) {
            ForEach(groups) { group in
                if let project = group.project {
                    Section(isExpanded: expansion(project)) {
                        rows(group.containers)
                    } header: {
                        ProjectHeader(project: project, containers: group.containers)
                    }
                } else {
                    Section(groups.count > 1 ? "Containers" : "") { rows(group.containers) }
                }
            }
        }
        .listStyle(.sidebar)
        .searchable(text: $query, placement: .sidebar, prompt: "Filter")
        .safeAreaInset(edge: .bottom, spacing: 0) { Footer() }
    }

    private func rows(_ containers: [Container]) -> some View {
        ForEach(containers) { container in
            ContainerRow(container: container)
                .tag(container.id)
                .transition(.asymmetric(insertion: .push(from: .leading), removal: .opacity.combined(with: .scale(scale: 0.9))))
        }
    }

    private func expansion(_ project: String) -> Binding<Bool> {
        Binding(
            get: { !collapsed.contains(project) },
            set: { expanded in withAnimation(.snappy) { if expanded { collapsed.remove(project) } else { collapsed.insert(project) } } }
        )
    }
}

private struct ProjectHeader: View {
    @Environment(DockerStore.self) private var store
    let project: String
    let containers: [Container]
    @State private var hovering = false

    private var running: Int { containers.filter(\.isRunning).count }

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "square.stack.3d.up.fill")
                .font(.system(size: 10))
            Text(project)
            Spacer()
            if hovering {
                Button {
                    store.perform(running > 0 ? .stop : .start, onProject: project)
                } label: {
                    Image(systemName: running > 0 ? "stop.fill" : "play.fill")
                        .contentTransition(.symbolEffect(.replace))
                }
                .buttonStyle(.plain)
                .help(running > 0 ? "Stop all" : "Start all")
                .transition(.opacity)
            } else {
                Text("\(running)/\(containers.count)")
                    .monospacedDigit()
                    .contentTransition(.numericText())
                    .transition(.opacity)
            }
        }
        .onHover { hover in withAnimation(.easeOut(duration: 0.15)) { hovering = hover } }
        .animation(.snappy, value: running)
    }
}

private struct ContainerRow: View {
    @Environment(DockerStore.self) private var store
    let container: Container
    @State private var hovering = false

    private var busy: ContainerAction? { store.busy[container.id] }

    var body: some View {
        HStack(spacing: 10) {
            StatusDot(state: container.state, busy: busy != nil)
            VStack(alignment: .leading, spacing: 1) {
                Text(container.service ?? container.name)
                    .font(.system(size: 13, weight: .medium))
                Text(subtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .contentTransition(.interpolate)
            }
            .lineLimit(1)
            Spacer(minLength: 4)
            ZStack(alignment: .trailing) {
                if hovering && busy == nil {
                    Button {
                        store.perform(container.isRunning ? .stop : .start, on: container)
                    } label: {
                        Image(systemName: container.isRunning ? "stop.fill" : "play.fill")
                            .font(.system(size: 11))
                            .frame(width: 22, height: 22)
                            .contentTransition(.symbolEffect(.replace))
                    }
                    .buttonStyle(HoverButtonStyle())
                    .help(container.isRunning ? "Stop" : "Start")
                    .transition(.blurReplace)
                } else if container.isRunning {
                    RowStats(stats: store.stats(for: container.id))
                        .transition(.blurReplace)
                }
            }
            .animation(.snappy(duration: 0.2), value: hovering)
        }
        .padding(.vertical, 4)
        .contentShape(.rect)
        .onHover { hovering = $0 }
        .contextMenu { ContainerMenu(container: container) }
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
