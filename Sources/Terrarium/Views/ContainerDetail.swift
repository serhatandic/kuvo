import SwiftUI

struct ContainerDetail: View {
    @Environment(DockerStore.self) private var store
    let container: Container
    @State private var details: ContainerDetails?
    @State private var confirmRemove = false

    private var busy: ContainerAction? { store.busy[container.id] }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            StatsRow(stats: store.stats(for: container.id), running: container.state == .running)
            LogsView(container: container)
        }
        .padding([.horizontal, .bottom], 20)
        .padding(.top, 8)
        // Name and status live in the toolbar, next to the actions, like Mail or Xcode.
        .navigationTitle(container.name)
        .background { Subtitle(container: container, details: details, busy: busy) }
        .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
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

// MARK: - Subtitle

/// "Running for 4 minutes · redis:alpine" under the window title, ticking live.
private struct Subtitle: View {
    let container: Container
    let details: ContainerDetails?
    let busy: ContainerAction?

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            Color.clear.navigationSubtitle("\(status(now: context.date))  ·  \(container.image)")
        }
    }

    private func status(now: Date) -> String {
        if let busy { return "\(busy.progressLabel)…" }
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

private struct StatsRow: View {
    let stats: LiveStats
    let running: Bool

    var body: some View {
        HStack(spacing: 12) {
            StatCard(
                title: "CPU", color: .blue, values: stats.cpu, tick: stats.tick, minimum: 5,
                value: stats.cpu.last ?? 0, text: Format.percent(stats.cpu.last ?? 0),
                detail: "of one core"
            )
            StatCard(
                title: "Memory", color: .purple, values: stats.memory, tick: stats.tick, minimum: 32 * 1024 * 1024,
                value: stats.memory.last ?? 0, text: Format.bytes(stats.memory.last ?? 0),
                detail: stats.memoryLimit > 0 ? "of \(Format.bytes(stats.memoryLimit))" : "",
                isLevel: true
            )
            StatCard(
                title: "Network", color: .teal, values: stats.network, tick: stats.tick, minimum: 2048,
                value: stats.rxRate + stats.txRate, text: Format.rate(stats.rxRate + stats.txRate),
                detail: "↓ \(Format.rate(stats.rxRate))  ↑ \(Format.rate(stats.txRate))"
            )
        }
        .opacity(running ? 1 : 0.55)
        .saturation(running ? 1 : 0)
        .animation(.smooth(duration: 0.5), value: running)
    }
}

private struct StatCard: View {
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
        Card {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline) {
                    Text(title)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .textCase(.uppercase)
                        .kerning(0.4)
                    Spacer()
                    Text(hasData ? detail : "")
                        .font(.system(size: 10.5).monospacedDigit())
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }
                Text(hasData ? text : "—")
                    .font(.system(size: 22, weight: .semibold, design: .rounded).monospacedDigit())
                    .contentTransition(.numericText(value: value))
                    .animation(.snappy, value: tick)
                Sparkline(values: values, tick: tick, minimum: minimum, color: color, isLevel: isLevel)
                    .frame(height: 38)
            }
        }
    }
}
