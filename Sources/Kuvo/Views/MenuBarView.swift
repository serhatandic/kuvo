import SwiftUI

/// The menu bar panel: every container, one click to start or stop.
struct MenuBarView: View {
    @Environment(DockerStore.self) private var store
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Kuvo Box").font(.system(size: 13, weight: .semibold))
                Spacer()
                let running = store.containers.filter(\.isRunning).count
                Text("\(running) running")
                    .font(.system(size: 11).monospacedDigit())
                    .foregroundStyle(.secondary)
                    .contentTransition(.numericText(value: Double(running)))
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 8)

            if store.containers.isEmpty {
                Text(store.connection == .connecting ? "Connecting…" : "No containers")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 20)
            } else {
                ScrollView {
                    VStack(spacing: 1) {
                        ForEach(store.containers) { MenuRow(container: $0) }
                    }
                    .padding(.horizontal, 6)
                }
                .frame(maxHeight: 340)
                .fixedSize(horizontal: false, vertical: true)
            }

            Divider().padding(.top, 6)
            HStack {
                Button("Open Kuvo Box") {
                    openWindow(id: "main")
                    NSApp.activate()
                }
                Spacer()
                Button("Quit") { NSApp.terminate(nil) }
            }
            .buttonStyle(HoverButtonStyle(tint: .secondary))
            .font(.system(size: 12))
            .padding(6)
        }
        .frame(width: 300)
        .animation(.snappy, value: store.containers)
    }
}

private struct MenuRow: View {
    @Environment(DockerStore.self) private var store
    let container: Container
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 9) {
            StatusDot(state: container.state, busy: store.busy[container.id] != nil, size: 7)
            Text(container.name)
                .font(.system(size: 12.5))
                .lineLimit(1)
            Spacer()
            Button {
                store.perform(container.isRunning ? .stop : .start, on: container)
            } label: {
                Image(systemName: container.isRunning ? "stop.fill" : "play.fill")
                    .font(.system(size: 10))
                    .contentTransition(.symbolEffect(.replace))
            }
            .buttonStyle(HoverButtonStyle(tint: .secondary))
            .opacity(hovering ? 1 : 0.5)
        }
        .padding(.leading, 8)
        .padding(.vertical, 1)
        .background(.primary.opacity(hovering ? 0.06 : 0), in: .rect(cornerRadius: 6))
        .onHover { hovering = $0 }
    }
}
