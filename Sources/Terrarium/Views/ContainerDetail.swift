import SwiftUI

struct ContainerDetail: View {
    @Environment(DockerStore.self) private var store
    let container: Container
    @State private var confirmRemove = false

    private var busy: ContainerAction? { store.busy[container.id] }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Header(container: container, busy: busy)
            StatsRow(stats: store.stats(for: container.id), running: container.state == .running)
            LogsView(container: container)
        }
        .padding(20)
        .toolbar {
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
        .confirmationDialog("Remove \(container.name)?", isPresented: $confirmRemove) {
            Button("Remove", role: .destructive) { store.perform(.remove, on: container) }
        } message: {
            Text(container.isRunning ? "The container will be stopped and deleted." : "The container will be deleted.")
        }
    }
}

// MARK: - Header

private struct Header: View {
    let container: Container
    let busy: ContainerAction?
    @State private var copied = false

    var body: some View {
        HStack(alignment: .center, spacing: 14) {
            ZStack {
                RoundedRectangle(cornerRadius: 11, style: .continuous)
                    .fill(container.state.color.gradient.opacity(container.isRunning ? 0.9 : 0.25))
                Image(systemName: "shippingbox.fill")
                    .font(.system(size: 20, weight: .medium))
                    .foregroundStyle(container.isRunning ? .white : .secondary)
                    .symbolEffect(.bounce, value: container.state)
            }
            .frame(width: 46, height: 46)
            .shadow(color: container.state.color.opacity(container.isRunning ? 0.35 : 0), radius: 8, y: 3)

            VStack(alignment: .leading, spacing: 4) {
                Text(container.name)
                    .font(.system(size: 20, weight: .semibold))
                    .lineLimit(1)
                HStack(spacing: 6) {
                    Text(container.image)
                    Text("·").foregroundStyle(.tertiary)
                    Button {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(container.id, forType: .string)
                        withAnimation(.snappy) { copied = true }
                        Task {
                            try? await Task.sleep(for: .seconds(1.4))
                            withAnimation(.snappy) { copied = false }
                        }
                    } label: {
                        HStack(spacing: 3) {
                            Text(copied ? "Copied" : container.shortID)
                                .contentTransition(.interpolate)
                            Image(systemName: copied ? "checkmark" : "doc.on.doc")
                                .font(.system(size: 9))
                                .contentTransition(.symbolEffect(.replace))
                        }
                        .font(.system(size: 12).monospaced())
                    }
                    .buttonStyle(.plain)
                    .help("Copy full ID")
                }
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .lineLimit(1)
            }

            Spacer(minLength: 12)

            HStack(spacing: 8) {
                ForEach(container.publishedPorts.prefix(3), id: \.self) { port in
                    PortChip(port: port, enabled: container.isRunning)
                }
                StatePill(container: container, busy: busy)
            }
        }
        .animation(.smooth, value: container.state)
    }
}

private struct StatePill: View {
    let container: Container
    let busy: ContainerAction?

    var body: some View {
        HStack(spacing: 6) {
            StatusDot(state: container.state, busy: busy != nil, size: 7)
            Text(busy.map { "\($0.progressLabel)…" } ?? container.status)
                .contentTransition(.interpolate)
        }
        .font(.system(size: 11.5, weight: .medium))
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background((busy != nil ? Color.orange : container.state.color).opacity(0.12), in: .capsule)
        .animation(.snappy, value: busy)
        .animation(.snappy, value: container.status)
    }
}

private struct PortChip: View {
    let port: Int
    let enabled: Bool
    @State private var hovering = false

    var body: some View {
        Button {
            NSWorkspace.shared.open(URL(string: "http://localhost:\(port)")!)
        } label: {
            HStack(spacing: 4) {
                Text(":\(String(port))").monospacedDigit()
                Image(systemName: "arrow.up.right")
                    .font(.system(size: 8, weight: .bold))
                    .offset(x: hovering ? 1 : 0, y: hovering ? -1 : 0)
            }
            .font(.system(size: 11.5, weight: .medium))
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(.primary.opacity(hovering ? 0.1 : 0.05), in: .capsule)
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .opacity(enabled ? 1 : 0.45)
        .onHover { hover in withAnimation(.snappy(duration: 0.2)) { hovering = hover } }
        .help("Open http://localhost:\(String(port))")
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
                detail: stats.memoryLimit > 0 ? "of \(Format.bytes(stats.memoryLimit))" : ""
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
                Sparkline(values: values, tick: tick, minimum: minimum, color: color)
                    .frame(height: 38)
            }
        }
    }
}
