import SwiftUI

struct ContainerDetail: View {
    @Environment(DockerStore.self) private var store
    @Environment(Terminals.self) private var terminals
    let container: Container
    @AppStorage("terminal.placement") private var placement = PanePlacement.trailing
    @AppStorage("terminal.fraction") private var fraction = 0.5
    @State private var maximized = false
    @State private var details: ContainerDetails?
    @State private var confirmRemove = false
    @State private var logs = LogModel()

    private var busy: ContainerAction? { store.busy[container.id] }
    private var archivedAt: Date? { Archive.archivedAt(container.id) }

    var body: some View {
        let session = terminals.sessions[container.id]
        PaneSplit(placement: placement, fraction: $fraction, showsSecond: session != nil, maximized: maximized) {
            VStack(alignment: .leading, spacing: 10) {
                StatsRow(stats: [store.stats(for: container.id)], running: container.state == .running)
                    .fixedSize(horizontal: false, vertical: true)
                panel
            }
        } second: {
            if let session {
                TerminalPane(container: container, session: session, placement: $placement, maximized: $maximized,
                             onRestart: { terminals.restart(container) },
                             onClose: { withAnimation(.smooth) { maximized = false; terminals.close(container.id) } })
            }
        }
        .padding([.horizontal, .bottom], 20)
        .padding(.top, 8)
        // Name and status live in the toolbar, next to the actions, like Mail or Xcode.
        .navigationTitle(container.name)
        .background { Subtitle(container: container, details: details, busy: busy, archivedAt: archivedAt) }
        .toolbar {
            ToolbarItemGroup {
                if let port = container.publishedPorts.first, container.isRunning {
                    Button {
                        NSWorkspace.shared.open(URL(string: "http://localhost:\(port)")!)
                    } label: {
                        Label(String("localhost:\(port)"), systemImage: "arrow.up.forward")
                            .labelStyle(.titleAndIcon)
                    }
                    .help("Open http://localhost:\(String(port))")
                }
            }
            ToolbarItemGroup {
                Button { withAnimation(.smooth) { terminals.toggle(container) } } label: {
                    Label("Terminal", systemImage: terminals.isOpen(container.id) ? "terminal.fill" : "terminal")
                        .contentTransition(.symbolEffect(.replace))
                }
                .help(terminals.isOpen(container.id) ? "Close terminal (⌃`)" : "Open a terminal (⌃`)")
                .keyboardShortcut("`", modifiers: .control)
                .disabled(!container.isRunning && !terminals.isOpen(container.id))

                Button {
                    store.perform(container.isRunning ? .stop : .start, on: container)
                } label: {
                    Label(container.isRunning ? "Stop" : "Start", systemImage: container.isRunning ? "stop.fill" : "play.fill")
                        .contentTransition(.symbolEffect(.replace))
                }
                .help(container.isRunning ? "Stop" : "Start")
                .keyboardShortcut(container.isRunning ? "." : "r", modifiers: .command)

                Button { store.perform(.restart, on: container) } label: {
                    Label("Restart", systemImage: "arrow.clockwise")
                        .symbolEffect(.rotate, value: busy == .restart)
                }
                .help("Restart")
                .disabled(!container.isRunning)

                if archivedAt != nil {
                    Button { store.restore(container) } label: { Label("Restore", systemImage: "arrow.uturn.backward") }
                        .help("Restore and start")
                } else {
                    Button { store.archive(container) } label: { Label("Archive", systemImage: "archivebox") }
                        .help("Stop and keep for now; deleted after the retention period")
                }

                Button { confirmRemove = true } label: { Label("Remove", systemImage: "trash") }
                    .help("Remove")
            }
        }
        .disabled(busy != nil)
        .task(id: "\(container.id)\(container.state)\(container.status)") {
            details = try? await store.client?.inspect(container.id)
        }
        .confirmationDialog("Remove \(container.name)?", isPresented: $confirmRemove) {
            Button("Remove", role: .destructive) { store.perform(.remove, on: container) }
        } message: {
            Text(container.isRunning ? "The container will be stopped and deleted." : "The container will be deleted.")
        }
    }
}

// MARK: - Panel

extension ContainerDetail {
    private var panel: some View {
        Pane(icon: "text.alignleft", title: "Logs") {
            LogsAccessory(model: logs)
        } content: {
            LogsBody(model: logs, sources: [LogSource(id: container.id)], isRunning: container.isRunning)
        }
    }
}

// MARK: - Subtitle

/// "Running for 4 minutes · redis:alpine" under the window title, ticking live.
private struct Subtitle: View {
    let container: Container
    let details: ContainerDetails?
    let busy: ContainerAction?
    let archivedAt: Date?

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let creator = container.creator.map { "  ·  by \($0)" } ?? ""
            Color.clear.navigationSubtitle("\(status(now: context.date))  ·  \(container.image)\(creator)")
        }
    }

    private func status(now: Date) -> String {
        if let busy { return "\(busy.progressLabel)…" }
        if let archivedAt { return ArchiveRetention.summary(archivedAt: archivedAt) }
        if container.state == .running, let start = details?.startedAt {
            let seconds = max(now.timeIntervalSince(start), 0)
            let fields: Set<Date.ComponentsFormatStyle.Field> = seconds < 60 ? [.second] : seconds < 3600 ? [.minute] : [.day, .hour, .minute]
            let uptime = (start..<max(now, start)).formatted(.components(style: .wide, fields: fields))
            guard let health = details?.State.Health?.Status, health != "none" else { return "Running for \(uptime)" }
            return "Running for \(uptime), \(health)"
        }
        if container.state == .exited, let details, let finished = details.finishedAt {
            let ago = finished.formatted(.relative(presentation: .named, unitsStyle: .wide))
            return details.State.ExitCode == 0 ? "Stopped \(ago)" : "Exited with code \(details.State.ExitCode), \(ago)"
        }
        return container.humanStatus
    }
}

// MARK: - Stats

/// CPU, memory and network for one container, or summed across several,
/// as one pane with three columns.
struct StatsRow: View {
    let stats: [LiveStats]
    let running: Bool

    var body: some View {
        let stats = StatsSeries(stats)
        Pane(icon: "waveform.path.ecg", title: "Resources") {
            EmptyView()
        } content: {
            HStack(spacing: 0) {
                Metric(
                    title: "CPU", color: .blue, values: stats.cpu, tick: stats.tick, minimum: 5,
                    value: stats.cpu.last ?? 0, text: Format.percent(stats.cpu.last ?? 0), detail: "of one core"
                )
                divider
                Metric(
                    title: "Memory", color: .purple, values: stats.memory, tick: stats.tick, minimum: 32 * 1024 * 1024,
                    value: stats.memory.last ?? 0, text: Format.bytes(stats.memory.last ?? 0),
                    detail: stats.memoryLimit > 0 ? "of \(Format.bytes(stats.memoryLimit))" : "", isLevel: true
                )
                divider
                Metric(
                    title: "Network", color: .teal, values: stats.network, tick: stats.tick, minimum: 2048,
                    value: stats.rxRate + stats.txRate, text: Format.rate(stats.rxRate + stats.txRate),
                    detail: "↓ \(Format.rate(stats.rxRate))  ↑ \(Format.rate(stats.txRate))"
                )
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 14)
            .opacity(running ? 1 : 0.5)
            .saturation(running ? 1 : 0)
            .animation(.smooth(duration: 0.5), value: running)
        }
    }

    private var divider: some View {
        Rectangle().fill(.separator.opacity(0.5)).frame(width: 0.5).padding(.vertical, 6).padding(.horizontal, 14)
    }
}

/// Series from one or more containers, summed and aligned on the newest sample.
private struct StatsSeries {
    var cpu: [Double] = []
    var memory: [Double] = []
    var network: [Double] = []
    var memoryLimit: Double = 0
    var rxRate: Double = 0
    var txRate: Double = 0
    var tick = 0

    @MainActor init(_ all: [LiveStats]) {
        for stats in all {
            cpu = Self.sum(cpu, stats.cpu)
            memory = Self.sum(memory, stats.memory, isLevel: true)
            network = Self.sum(network, stats.network)
            memoryLimit = max(memoryLimit, stats.memoryLimit)
            rxRate += stats.rxRate
            txRate += stats.txRate
            tick = max(tick, stats.tick)
        }
    }

    /// Levels (memory) extend a shorter history with its first sample; rates with zero.
    private static func sum(_ a: [Double], _ b: [Double], isLevel: Bool = false) -> [Double] {
        let (long, short) = a.count >= b.count ? (a, b) : (b, a)
        let offset = long.count - short.count
        let fill = isLevel ? short.first ?? 0 : 0
        return long.enumerated().map { i, v in v + (i >= offset ? short[i - offset] : fill) }
    }
}

private struct Metric: View {
    let title: String
    let color: Color
    let values: [Double]
    let tick: Int
    let minimum: Double
    let value: Double
    let text: String
    let detail: String
    var isLevel = false

    private var hasData: Bool { !values.isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(.secondary)
            Text(hasData ? text : "—")
                .font(.system(size: 20, weight: .semibold, design: .rounded).monospacedDigit())
                .contentTransition(.numericText(value: value))
                .animation(.snappy, value: tick)
            Text(hasData ? detail : " ")
                .font(.system(size: 11).monospacedDigit())
                .foregroundStyle(.tertiary)
                .lineLimit(1)
            Sparkline(values: values, tick: tick, minimum: minimum, color: color, isLevel: isLevel)
                .frame(height: 34)
                .padding(.top, 8)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
