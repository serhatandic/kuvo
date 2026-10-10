import SwiftUI

/// A Compose project at a glance: combined stats and every service's logs, merged.
struct ProjectDetail: View {
    @Environment(DockerStore.self) private var store
    let project: String

    private var containers: [Container] { store.containers.filter { $0.project == project } }
    private var running: [Container] { containers.filter(\.isRunning) }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            StatsRow(stats: running.map { store.stats(for: $0.id) }, running: !running.isEmpty)
            LogsView(
                sources: containers.map { LogSource(id: $0.id, label: $0.service ?? $0.name) },
                isRunning: !running.isEmpty
            )
        }
        .padding([.horizontal, .bottom], 20)
        .padding(.top, 8)
        .navigationTitle(project)
        .navigationSubtitle("Compose project  ·  \(summary)")
        .toolbar {
            ToolbarItemGroup {
                Button {
                    store.perform(running.isEmpty ? .start : .stop, onProject: project)
                } label: {
                    Label(running.isEmpty ? "Start All" : "Stop All", systemImage: running.isEmpty ? "play.fill" : "stop.fill")
                        .contentTransition(.symbolEffect(.replace))
                }
                .help(running.isEmpty ? "Start all services" : "Stop all services")

                Button { store.perform(.restart, onProject: project) } label: {
                    Label("Restart All", systemImage: "arrow.clockwise")
                }
                .help("Restart all services")
                .disabled(running.isEmpty)
            }
        }
    }

    private var summary: String {
        switch running.count {
        case 0: "\(containers.count) services, all stopped"
        case containers.count: "\(containers.count) services, all running"
        default: "\(running.count) of \(containers.count) services running"
        }
    }
}
