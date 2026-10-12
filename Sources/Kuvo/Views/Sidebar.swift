import SwiftUI

/// What the sidebar lists. Each section keeps its own selection and filter.
enum SidebarSection: String, CaseIterable {
    case containers, volumes

    var title: String {
        switch self {
        case .containers: "Containers"
        case .volumes: "Volumes"
        }
    }

    var symbol: String {
        switch self {
        case .containers: "shippingbox"
        case .volumes: "cylinder"
        }
    }

    /// ⌘1, ⌘2… in order, like Xcode's navigators.
    var shortcut: KeyEquivalent {
        KeyEquivalent(Character(String(Self.allCases.firstIndex(of: self)! + 1)))
    }
}

/// One kind of thing at a time, picked in the icon bar at the top. In the
/// container list, Compose projects are collapsible groups and everything else
/// is a plain row.
struct Sidebar: View {
    @Environment(DockerStore.self) private var store
    @Binding var section: SidebarSection
    @Binding var selection: String?
    @Binding var volumeSelection: String?
    /// Per section, so filtering containers for "redis" doesn't empty the volume list.
    @State private var queries: [SidebarSection: String] = [:]
    @State private var collapsed: Set<String> = []

    private var query: String { queries[section] ?? "" }

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

    private func matches(_ container: Container) -> Bool {
        query.isEmpty || container.name.localizedCaseInsensitiveContains(query) || container.image.localizedCaseInsensitiveContains(query)
    }

    private var items: [Item] {
        let matches = store.containers.filter(matches)
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
        VStack(spacing: 8) {
            SectionBar(section: $section)
                .padding(.horizontal, 10)
            FilterField(text: Binding(get: { query }, set: { queries[section] = $0 }))
                // A field being edited keeps its own text; a fresh one per section shows that section's filter.
                .id(section)
                .padding(.horizontal, 10)
            ZStack {
                switch section {
                case .containers: list
                case .volumes: VolumeList(selection: $volumeSelection, query: query)
                }
            }
            .id(section)
            .transition(.opacity)
        }
        .animation(.easeOut(duration: 0.15), value: section)
    }

    private var list: some View {
        List(selection: $selection) {
            ForEach(items) { item in
                switch item {
                case .project(let name, let containers):
                    ProjectRow(project: name, containers: containers, expanded: expansion(name))
                        .tag("project:\(name)")
                    if expansion(name).wrappedValue {
                        ForEach(containers) { row($0).padding(.leading, 16) }
                    }
                case .container(let container):
                    row(container)
                }
            }
        }
        .sidebarList()
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

/// The sections as a row of icons, like Xcode's navigator bar: it stays one row
/// as sections are added, where text labels stop fitting. The names are in the
/// tooltips, the View menu and VoiceOver.
private struct SectionBar: View {
    @Binding var section: SidebarSection

    var body: some View {
        HStack(spacing: 2) {
            ForEach(SidebarSection.allCases, id: \.self) { item in
                SectionButton(item: item, selected: item == section) { section = item }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Show")
    }
}

private struct SectionButton: View {
    let item: SidebarSection
    let selected: Bool
    let select: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: select) {
            Image(systemName: selected ? "\(item.symbol).fill" : item.symbol)
                .font(.system(size: 14))
                .foregroundStyle(selected ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
                .frame(maxWidth: .infinity)
                .frame(height: 28)
                .background(.primary.opacity(hovering && !selected ? 0.06 : 0), in: .rect(cornerRadius: 7))
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .help("\(item.title) (⌘\(String(item.shortcut.character)))")
        .accessibilityLabel(item.title)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

extension View {
    /// The sidebar's list style, shared by every section.
    func sidebarList() -> some View { modifier(SidebarListStyle()) }
}

private struct SidebarListStyle: ViewModifier {
    @Environment(\.colorScheme) private var scheme

    func body(content: Content) -> some View {
        content
            .listStyle(.sidebar)
            // Light mode: let the window's tinted glass show instead of the gray sidebar material.
            .scrollContentBackground(scheme == .light ? .hidden : .automatic)
    }
}

/// The sidebar's filter: a search field sized to sit comfortably above the rows.
private struct FilterField: View {
    @Binding var text: String
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: 7) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
            TextField("Filter", text: $text)
                .textFieldStyle(.plain)
                .font(.system(size: 13))
                .focused($focused)
                .onExitCommand { text = "" }
            if !text.isEmpty {
                Button { text = "" } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
                .help("Clear")
                .transition(.opacity)
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 28)
        .background(.primary.opacity(0.06), in: .rect(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .strokeBorder(focused ? Color.accentColor.opacity(0.6) : .primary.opacity(0.08), lineWidth: focused ? 1.5 : 0.5)
        }
        .contentShape(.rect)
        .onTapGesture { focused = true }
        .animation(.easeOut(duration: 0.15), value: focused)
        .animation(.easeOut(duration: 0.15), value: text.isEmpty)
    }
}

// MARK: - Rows

/// Shared layout for every sidebar row: a 16pt leading glyph, two lines of
/// text, and a fixed trailing slot that swaps to an action on hover without
/// changing the row's size.
struct RowLayout<Glyph: View, Trailing: View>: View {
    let title: String
    let subtitle: String
    let action: RowAction?
    /// Room for the trailing view: container rows show a CPU graph, volume rows a size.
    var trailingWidth: CGFloat = 84
    /// Receives whether the row is hovered, so a glyph can reveal a control.
    @ViewBuilder var glyph: (_ hovering: Bool) -> Glyph
    @ViewBuilder var trailing: Trailing
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 8) {
            glyph(hovering).frame(width: 16)
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
            .frame(width: trailingWidth, height: 24, alignment: .trailing)
            .animation(.easeOut(duration: 0.12), value: hovering)
        }
        .padding(.vertical, 3)
        .contentShape(.rect)
        .onHover { hovering = $0 }
    }
}

/// A real, comfortably sized button for row actions (not a bare glyph).
struct RowAction: View {
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

/// A group's icon, which turns into its disclosure chevron while hovered.
private struct GroupGlyph: View {
    let icon: String
    @Binding var expanded: Bool
    let hovering: Bool

    var body: some View {
        Button { withAnimation(.snappy) { expanded.toggle() } } label: {
            ZStack {
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .bold))
                    .rotationEffect(.degrees(expanded ? 90 : 0))
                    .opacity(hovering ? 1 : 0)
                    .scaleEffect(hovering ? 1 : 0.6)
                Image(systemName: icon)
                    .font(.system(size: 12))
                    .opacity(hovering ? 0 : 1)
                    .scaleEffect(hovering ? 0.6 : 1)
            }
            .foregroundStyle(.secondary)
            .frame(width: 16, height: 24)
            .contentShape(.rect)
            .animation(.snappy(duration: 0.18), value: hovering)
        }
        .buttonStyle(.plain)
        .help(expanded ? "Collapse" : "Expand")
    }
}

private struct ProjectRow: View {
    @Environment(DockerStore.self) private var store
    let project: String
    let containers: [Container]
    @Binding var expanded: Bool
    @State private var confirmRemove = false

    private var running: Int { containers.filter(\.isRunning).count }

    var body: some View {
        RowLayout(title: project, subtitle: subtitle, action: action) { hovering in
            GroupGlyph(icon: "square.stack.3d.up", expanded: $expanded, hovering: hovering)
        } trailing: {
            ProjectCPU(stats: containers.filter(\.isRunning).map { store.stats(for: $0.id) })
        }
        .contextMenu {
            Button("Start All") { store.perform(.start, onProject: project) }.disabled(running == containers.count)
            Button("Stop All") { store.perform(.stop, onProject: project) }.disabled(running == 0)
            Divider()
            Button("Remove…", role: .destructive) { confirmRemove = true }
        }
        .confirmRemove(project: project, count: containers.count, isPresented: $confirmRemove)
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
    }
}

private struct ContainerRow: View {
    @Environment(DockerStore.self) private var store
    let container: Container

    private var busy: ContainerAction? { store.busy[container.id] }

    var body: some View {
        RowLayout(title: container.service ?? container.name, subtitle: subtitle, action: action) { _ in
            if archivedAt != nil {
                Image(systemName: "archivebox").font(.system(size: 11)).foregroundStyle(.tertiary)
            } else {
                StatusDot(state: container.state, busy: busy != nil)
            }
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

    private var archivedAt: Date? { Archive.archivedAt(container.id) }

    private var subtitle: String {
        if let busy { return "\(busy.progressLabel)…" }
        if let archivedAt { return ArchiveRetention.summary(archivedAt: archivedAt) }
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
        }
        .opacity(stats.hasData ? 1 : 0)
        .animation(.easeOut(duration: 0.4), value: stats.hasData)
    }
}

struct ContainerMenu: View {
    @Environment(DockerStore.self) private var store
    @Environment(Terminals.self) private var terminals
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
        if container.isRunning {
            Divider()
            Button("Open Terminal") { withAnimation(.smooth) { terminals.open(container) } }
            Button("Open in Terminal App") { TerminalLauncher.open(container) }
            if let port = container.publishedPorts.first {
                Button("Open localhost:\(String(port))") { NSWorkspace.shared.open(URL(string: "http://localhost:\(port)")!) }
            }
        }
        Divider()
        Button("Copy ID") { copy(container.id) }
        Button("Copy Name") { copy(container.name) }
        Divider()
        if Archive.archivedAt(container.id) != nil {
            Button("Restore") { store.restore(container) }
        } else {
            Button("Archive") { store.archive(container) }
        }
        Button("Remove", role: .destructive) { store.perform(.remove, on: container) }
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

extension FocusedValues {
    /// The front window's sidebar section, for the View menu.
    @Entry var sidebarSection: Binding<SidebarSection>?
}

/// View › Containers ⌘1, Volumes ⌘2…, checked for the section showing.
struct SectionCommands: View {
    @FocusedBinding(\.sidebarSection) private var section

    var body: some View {
        ForEach(SidebarSection.allCases, id: \.self) { item in
            Toggle(item.title, isOn: Binding(get: { section == item }, set: { if $0 { section = item } }))
                .keyboardShortcut(item.shortcut, modifiers: .command)
        }
        .disabled(section == nil)
    }
}
