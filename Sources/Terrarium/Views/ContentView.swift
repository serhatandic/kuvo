import SwiftUI

struct ContentView: View {
    @Environment(DockerStore.self) private var store
    @SceneStorage("selection") private var selection: String?
    @State private var columns = NavigationSplitViewVisibility.all
    @State private var sidebarWidth: CGFloat = 0

    private var selected: Container? {
        store.containers.first { $0.id == selection }
    }

    /// Projects are selected as `project:<name>`.
    private var selectedProject: String? {
        guard let selection, selection.hasPrefix("project:") else { return nil }
        let name = String(selection.dropFirst("project:".count))
        return store.containers.contains { $0.project == name } ? name : nil
    }

    var body: some View {
        ZStack {
            switch store.connection {
            case .connected:
                NavigationSplitView(columnVisibility: $columns) {
                    Sidebar(selection: $selection)
                        .navigationSplitViewColumnWidth(min: 250, ideal: 290, max: 420)
                        .onGeometryChange(for: CGFloat.self, of: { $0.frame(in: .global).maxX }) { sidebarWidth = $0 }
                } detail: {
                    detail
                }
                .overlay(alignment: .topLeading) {
                    // The system divider is pure black in dark mode and reads as heavy on
                    // translucent panes; cover it with a soft seam. Dragging still works.
                    if columns != .detailOnly, sidebarWidth > 0 {
                        Rectangle()
                            .fill(.thinMaterial)
                            .overlay(Color.primary.opacity(0.1))
                            .frame(width: 1)
                            .offset(x: sidebarWidth)
                            .ignoresSafeArea()
                            .allowsHitTesting(false)
                    }
                }
                .transition(.opacity)
            case .connecting, .unavailable:
                EngineStatusView()
                    .toolbar(removing: .title)
                    .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
                    .transition(.opacity.combined(with: .scale(0.98)))
            }
        }
        .frame(minWidth: 820, minHeight: 520)
        .containerBackground(.thinMaterial, for: .window)
        .alert("Something went wrong", isPresented: Binding(get: { store.failure != nil }, set: { if !$0 { store.failure = nil } })) {
            Button("OK") {}
        } message: {
            Text(store.failure ?? "")
        }
        .onChange(of: store.containers.map(\.id)) { _, ids in
            if selectedProject == nil, selection == nil || !ids.contains(selection!) {
                withAnimation(.snappy) { selection = store.containers.first(where: \.isRunning)?.id ?? ids.first }
            }
        }
    }

    @ViewBuilder private var detail: some View {
        ZStack {
            if let selected {
                ContainerDetail(container: selected)
                    .id(selected.id)
                    .transition(.opacity.combined(with: .offset(y: 6)))
            } else if let selectedProject {
                ProjectDetail(project: selectedProject)
                    .id(selectedProject)
                    .transition(.opacity.combined(with: .offset(y: 6)))
            } else if store.containers.isEmpty {
                EmptyStateView().transition(.opacity)
            } else {
                ContentUnavailableView("No Selection", systemImage: "shippingbox", description: Text("Pick a container from the sidebar."))
                    .transition(.opacity)
            }
        }
        .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        .animation(.snappy(duration: 0.28), value: selection)
    }
}

private struct EmptyStateView: View {
    @State private var copied = false
    private let command = "docker run -d -p 8080:80 --name hello nginx:alpine"

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "shippingbox")
                .font(.system(size: 44, weight: .light))
                .foregroundStyle(.tertiary)
                .symbolEffect(.bounce, options: .nonRepeating)
            Text("No containers yet")
                .font(.system(size: 17, weight: .semibold))
            Text("Run something and it shows up here instantly.")
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(command, forType: .string)
                withAnimation(.snappy) { copied = true }
            } label: {
                HStack(spacing: 8) {
                    Text(command).font(.system(size: 12).monospaced())
                    Image(systemName: copied ? "checkmark" : "doc.on.doc")
                        .contentTransition(.symbolEffect(.replace))
                        .foregroundStyle(copied ? .green : .secondary)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(.background.secondary, in: .rect(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
            }
            .buttonStyle(.plain)
            .padding(.top, 6)
        }
        .padding(40)
    }
}

/// Shown until Docker answers: first-run download, install steps, boot, errors.
private struct EngineStatusView: View {
    @Environment(Engine.self) private var engine

    var body: some View {
        VStack(spacing: 14) {
            icon
                .font(.system(size: 46, weight: .light))
                .frame(height: 56)
            Text(title)
                .font(.system(size: 17, weight: .semibold))
                .contentTransition(.interpolate)
            detail
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 340)
                .contentTransition(.interpolate)
            actions.padding(.top, 4)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .animation(.smooth, value: engine.state)
    }

    @ViewBuilder private var icon: some View {
        switch engine.state {
        case .failed:
            Image(systemName: "exclamationmark.triangle").foregroundStyle(.orange)
        case .stopped:
            Image(systemName: "shippingbox").foregroundStyle(.tertiary)
        default:
            Image(systemName: "shippingbox")
                .foregroundStyle(.tertiary)
                .symbolEffect(.pulse, options: .repeating)
        }
    }

    private var title: String {
        switch engine.state {
        case .downloading: "Downloading Linux"
        case .booting(let step): step
        case .running: "Connecting"
        case .stopping: "Stopping"
        case .stopped: "Engine stopped"
        case .failed: "Engine didn't start"
        }
    }

    @ViewBuilder private var detail: some View {
        switch engine.state {
        case .downloading(let progress):
            VStack(spacing: 10) {
                ProgressView(value: progress).frame(width: 220)
                Text("A one-time \(Format.bytes(Double(GuestImage.assets.reduce(0) { $0 + $1.size }))) download of Alpine Linux \(GuestImage.alpine).")
            }
        case .booting(let step) where step != "Starting":
            Text("First launch sets up a small Linux VM with Docker. This takes about a minute.")
        case .failed(let reason):
            Text(reason)
        case .stopped:
            Text("Start it to run containers.")
        default:
            EmptyView()
        }
    }

    @ViewBuilder private var actions: some View {
        switch engine.state {
        case .failed:
            HStack {
                Button("Show Log") { NSWorkspace.shared.activateFileViewerSelecting([EnginePaths.console]) }
                Button("Try Again") { Task { await engine.start() } }.keyboardShortcut(.defaultAction)
            }
        case .stopped:
            Button("Start Engine") { Task { await engine.start() } }.keyboardShortcut(.defaultAction)
        default:
            EmptyView()
        }
    }
}
