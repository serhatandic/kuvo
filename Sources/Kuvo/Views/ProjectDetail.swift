import SwiftUI

/// A Compose project at a glance: combined stats and every service's logs, merged.
struct ProjectDetail: View {
    @Environment(DockerStore.self) private var store
    let project: String
    @State private var confirmRemove = false

    private var containers: [Container] { store.containers.filter { $0.project == project } }
    private var running: [Container] { containers.filter(\.isRunning) }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            StatsRow(stats: running.map { store.stats(for: $0.id) }, running: !running.isEmpty)
                .fixedSize(horizontal: false, vertical: true)
            LogsView(
                sources: containers.map { LogSource(id: $0.id, label: $0.service ?? $0.name) },
                isRunning: !running.isEmpty
            )
        }
        .padding([.horizontal, .bottom], 20)
        .padding(.top, 8)
        .navigationTitle(project)
        .navigationSubtitle(summary)
        .toolbar {
            ToolbarItem(placement: .navigation) { TitleIcon(systemName: "square.stack.3d.up.fill") }
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

                Button { confirmRemove = true } label: { Label("Remove", systemImage: "trash") }
                    .help("Remove all services")
            }
        }
        .confirmRemove(project: project, count: containers.count, isPresented: $confirmRemove)
    }

    private var summary: String {
        switch running.count {
        case 0: containers.count == 1 ? "1 service, stopped" : "\(containers.count) services, all stopped"
        case containers.count: containers.count == 1 ? "1 service, running" : "\(containers.count) services, all running"
        default: "\(running.count) of \(containers.count) services running"
        }
    }
}

extension View {
    /// Asks before removing every container in a Compose project.
    func confirmRemove(project: String, count: Int, isPresented: Binding<Bool>) -> some View {
        modifier(RemoveProjectConfirmation(project: project, count: count, isPresented: isPresented))
    }
}

private struct RemoveProjectConfirmation: ViewModifier {
    @Environment(DockerStore.self) private var store
    let project: String
    let count: Int
    @Binding var isPresented: Bool

    func body(content: Content) -> some View {
        content.confirmationDialog("Remove \(project)?", isPresented: $isPresented) {
            Button("Remove", role: .destructive) { store.perform(.remove, onProject: project) }
        } message: {
            Text(count == 1
                 ? "Its container will be stopped and deleted. Volumes are kept."
                 : "Its \(count) containers will be stopped and deleted. Volumes are kept.")
        }
    }
}
