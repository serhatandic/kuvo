import SwiftUI

struct ContentView: View {
    @Environment(DockerStore.self) private var store
    @SceneStorage("selection") private var selection: String?

    private var selected: Container? {
        store.containers.first { $0.id == selection }
    }

    var body: some View {
        ZStack {
            switch store.connection {
            case .connected:
                NavigationSplitView {
                    Sidebar(selection: $selection)
                        .navigationSplitViewColumnWidth(min: 250, ideal: 290, max: 420)
                } detail: {
                    detail
                }
                .transition(.opacity)
            case .connecting:
                ProgressView().controlSize(.small).transition(.opacity)
            case .unavailable(let reason):
                DisconnectedView(reason: reason).transition(.opacity.combined(with: .scale(0.98)))
            }
        }
        .frame(minWidth: 820, minHeight: 520)
        .toolbar(removing: .title)
        .alert("Something went wrong", isPresented: Binding(get: { store.failure != nil }, set: { if !$0 { store.failure = nil } })) {
            Button("OK") {}
        } message: {
            Text(store.failure ?? "")
        }
        .onChange(of: store.containers.map(\.id)) { _, ids in
            if selection == nil || !ids.contains(selection!) {
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
            } else if store.containers.isEmpty {
                EmptyStateView().transition(.opacity)
            } else {
                ContentUnavailableView("No Selection", systemImage: "shippingbox", description: Text("Pick a container from the sidebar."))
                    .transition(.opacity)
            }
        }
        .animation(.snappy(duration: 0.28), value: selected?.id)
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

private struct DisconnectedView: View {
    let reason: String

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "shippingbox")
                .font(.system(size: 48, weight: .light))
                .foregroundStyle(.tertiary)
                .symbolEffect(.pulse, options: .repeating)
            Text("Docker isn't running")
                .font(.system(size: 17, weight: .semibold))
            Text("Start Docker Desktop, OrbStack or Colima.\nTerrarium connects automatically.")
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            if let docker = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.docker.docker") {
                Button("Open Docker Desktop") { NSWorkspace.shared.open(docker) }
                    .controlSize(.large)
                    .padding(.top, 6)
            }
            Text(reason)
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
                .padding(.top, 12)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
