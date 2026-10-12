import SwiftUI

struct ContentView: View {
    @Environment(DockerStore.self) private var store
    @Environment(Navigator.self) private var navigator
    @SceneStorage("section") private var section = SidebarSection.containers
    @SceneStorage("selection") private var selection: String?
    @SceneStorage("imageSelection") private var imageSelection: String?
    @SceneStorage("volumeSelection") private var volumeSelection: String?
    @State private var columns = NavigationSplitViewVisibility.all
    @State private var sidebarWidth: CGFloat = 0
    @AppStorage(Appearance.key) private var appearance = Appearance.system
    @Environment(\.colorScheme) private var scheme

    private var selected: Container? {
        store.containers.first { $0.id == selection }
    }

    /// Projects are selected as `project:<name>`.
    private var selectedProject: String? {
        guard let selection, selection.hasPrefix("project:") else { return nil }
        let name = String(selection.dropFirst("project:".count))
        return store.containers.contains { $0.project == name } ? name : nil
    }

    /// False while the window is closed, minimized or covered. Live stats (which drive
    /// every graph and number) only stream while it's on screen.
    @State private var onScreen = true
    /// Offered once, the first time the engine is up.
    @State private var askToUseKuvo = false

    var body: some View {
        content
            .frame(minWidth: 820, minHeight: 520)
            .background(WindowVisibilityReader(isVisible: $onScreen))
            .onChange(of: onScreen, initial: true) { store.wantsLiveStats = onScreen }
            .onChange(of: navigator.section) {
                guard let requested = navigator.section else { return }
                withAnimation(.snappy) { section = requested }
                navigator.section = nil
            }
            .onChange(of: [onScreen, section == .volumes], initial: true) { store.wantsVolumes = onScreen && section == .volumes }
            .onChange(of: appearance, initial: true) { appearance.apply() }
            .onChange(of: store.connection, initial: true) {
                // Only installed copies ask; development builds leave docker alone.
                if case .connected = store.connection, CLIInstaller.appBundle != nil, DockerContext.shouldAsk {
                    askToUseKuvo = true
                }
            }
            .alert("Use Kuvo for the docker command?", isPresented: $askToUseKuvo) {
                Button("Use Kuvo") {
                    DockerContext.markAsked()
                    do { try DockerContext.use() } catch { store.failure = error.localizedDescription }
                }
                .keyboardShortcut(.defaultAction)
                Button("Not Now", role: .cancel) { DockerContext.markAsked() }
            } message: {
                Text(DockerContext.hasDockerDesktop
                    ? "docker, Docker Compose and other tools will use Kuvo in every terminal.\n\nIt's easy to undo: opening Docker Desktop switches them back to it."
                    : "docker, Docker Compose and other tools will use Kuvo in every terminal.\n\nTo undo it, run: docker context use default")
            }
            // Frosted and tinted by the wallpaper; light mode adds a tinted layer on top (see lightGlass).
            .containerBackground(.thinMaterial, for: .window)
    }

    @ViewBuilder private var content: some View {
        ZStack {
            switch store.connection {
            case .connected:
                NavigationSplitView(columnVisibility: $columns) {
                    Sidebar(section: $section, selection: $selection, imageSelection: $imageSelection, volumeSelection: $volumeSelection)
                        .lightGlass() // The sidebar draws its own material; tint it the same.
                        // Also as a frame: without saved split state, the column width alone is ignored.
                        // No maximum here: the contents fill the column, and SidebarWidthLimits caps it.
                        .frame(minWidth: 250, idealWidth: 290, maxWidth: .infinity)
                        .navigationSplitViewColumnWidth(min: 250, ideal: 290, max: 420)
                        .background(SidebarWidthLimits(min: 250, max: 420))
                        .onGeometryChange(for: CGFloat.self, of: { $0.frame(in: .global).maxX }) { sidebarWidth = $0 }
                } detail: {
                    detail
                }
                // One continuous sheet of tinted glass behind sidebar and detail (light mode).
                .lightGlass()
                .focusedSceneValue(\.sidebarSection, $section)
                .overlay(alignment: .topLeading) {
                    // The system divider is pure black in dark mode and reads as heavy on
                    // translucent panes; cover it with a soft seam. Dragging still works.
                    if columns != .detailOnly, sidebarWidth > 0 {
                        Rectangle()
                            .fill(.thinMaterial)
                            .overlay(Color.primary.opacity(scheme == .light ? 0.06 : 0.1))
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
        switch section {
        case .containers:
            containerDetail
        case .images:
            ImagesDetail(selection: $imageSelection, openContainer: open)
                .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        case .volumes:
            VolumesDetail(selection: $volumeSelection, openContainer: open)
                .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        }
    }

    /// From an image or volume to a container that uses it.
    private func open(_ container: Container) {
        withAnimation(.snappy) {
            section = .containers
            selection = container.id
        }
    }

    @ViewBuilder private var containerDetail: some View {
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
    @Environment(DockerStore.self) private var store
    /// Re-checked when Kuvo comes to the front, in case the context changed in a terminal.
    @State private var usesKuvo = DockerContext.current == DockerContext.name
    /// Once the steps are showing they stay, so finishing step 1 doesn't rearrange the view.
    @State private var showsSteps = DockerContext.current != DockerContext.name
    private let command = "docker run -d -p 8080:80 --name hello nginx:alpine"

    var body: some View {
        VStack(spacing: 14) {
            if !showsSteps {
                Text("Run something and it shows up here instantly.")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                CommandChip(command: command).padding(.top, 6)
            } else {
                VStack(alignment: .leading, spacing: 18) {
                    Step(number: 1, title: "Point the docker command at Kuvo", done: usesKuvo) {
                        Button {
                            do {
                                try DockerContext.use()
                                withAnimation(.snappy) { usesKuvo = true }
                            } catch {
                                store.failure = error.localizedDescription
                            }
                        } label: {
                            Label(usesKuvo ? "Docker uses Kuvo" : "Use Kuvo for docker",
                                  systemImage: usesKuvo ? "checkmark" : "arrow.right")
                                .labelStyle(StepButtonLabelStyle(showsIcon: usesKuvo))
                                .contentTransition(.interpolate)
                        }
                        .controlSize(.large)
                        .buttonStyle(.borderedProminent)
                        .disabled(usesKuvo)
                        Text(DockerContext.hasDockerDesktop
                            ? "It's easy to undo: opening Docker Desktop switches docker back to it."
                            : "To undo it, run: docker context use default")
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                    }
                    Step(number: 2, title: "Run something") {
                        CommandChip(command: command)
                    }
                }
                .padding(.top, 8)
            }
        }
        .padding(40)
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            let current = DockerContext.current == DockerContext.name
            if current != usesKuvo { withAnimation(.snappy) { usesKuvo = current } }
        }
    }
}

/// The button's checkmark only appears once the step is done.
private struct StepButtonLabelStyle: LabelStyle {
    let showsIcon: Bool

    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 6) {
            if showsIcon { configuration.icon.transition(.scale.combined(with: .opacity)) }
            configuration.title
        }
    }
}

/// A numbered step in the empty state; its number turns into a checkmark when done.
private struct Step<Content: View>: View {
    let number: Int
    let title: String
    var done = false
    @ViewBuilder var content: Content

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            ZStack {
                if done {
                    Image(systemName: "checkmark")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundStyle(.white)
                        .transition(.scale.combined(with: .opacity))
                } else {
                    Text("\(number)")
                        .font(.system(size: 12, weight: .semibold).monospacedDigit())
                        .foregroundStyle(.secondary)
                        .transition(.opacity)
                }
            }
            .frame(width: 22, height: 22)
            .background(done ? AnyShapeStyle(Color.green) : AnyShapeStyle(.primary.opacity(0.07)), in: .circle)
            .animation(.snappy, value: done)
            VStack(alignment: .leading, spacing: 8) {
                Text(title).font(.system(size: 13, weight: .medium))
                content
            }
        }
    }
}

/// A command shown in monospace that copies itself when clicked.
private struct CommandChip: View {
    let command: String
    @State private var copied = false

    var body: some View {
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
        .help("Copy")
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

/// Keeps the sidebar between `min` and `max` points wide. SwiftUI's column limits
/// stop holding once macOS restores a saved divider position, and SwiftUI also
/// resets limits set on the AppKit split view item, so this checks after every
/// resize and moves the divider back when the sidebar is too wide.
private struct SidebarWidthLimits: NSViewRepresentable {
    let min: CGFloat
    let max: CGFloat

    func makeNSView(context: Context) -> Probe { Probe(min: min, max: max) }
    func updateNSView(_ view: Probe, context: Context) {}

    final class Probe: NSView {
        private let min: CGFloat
        private let max: CGFloat
        private weak var splitView: NSSplitView?
        private var observer: NSObjectProtocol?

        init(min: CGFloat, max: CGFloat) {
            self.min = min
            self.max = max
            super.init(frame: .zero)
        }

        required init?(coder: NSCoder) { fatalError() }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard window != nil else {
                observer.map(NotificationCenter.default.removeObserver)
                observer = nil
                splitView = nil
                return
            }
            // The split view finishes assembling after this view joins the window.
            DispatchQueue.main.async { [weak self] in self?.attach() }
        }

        private func attach() {
            var view = superview
            while let current = view, !(current is NSSplitView) { view = current.superview }
            guard let split = view as? NSSplitView, split !== splitView else { return }
            splitView = split
            observer.map(NotificationCenter.default.removeObserver)
            observer = NotificationCenter.default.addObserver(
                forName: NSSplitView.didResizeSubviewsNotification, object: split, queue: .main
            ) { [weak self] _ in MainActor.assumeIsolated { self?.enforce() } }
            enforce()
        }

        private func enforce() {
            guard let split = splitView, let sidebar = split.arrangedSubviews.first, !sidebar.isHidden else { return }
            if let item = (split.delegate as? NSSplitViewController)?.splitViewItems.first {
                item.minimumThickness = min
                item.maximumThickness = max
            }
            if sidebar.frame.width > max + 0.5 {
                split.setPosition(max, ofDividerAt: 0)
            }
        }
    }
}
