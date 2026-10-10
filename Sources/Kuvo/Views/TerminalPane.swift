import AppKit
import SwiftTerm
import SwiftUI

// MARK: - Sessions

/// Live shells, one per container, kept while you look at other containers.
@MainActor @Observable
final class Terminals {
    private(set) var sessions: [String: TerminalSession] = [:]
    @ObservationIgnored private var appearanceObserver: NSKeyValueObservation?

    init() {
        // SwiftTerm stores colors as fixed RGB, so re-apply them when light/dark changes.
        appearanceObserver = NSApplication.shared.observe(\.effectiveAppearance) { [weak self] _, _ in
            Task { @MainActor in self?.sessions.values.forEach { $0.restyle() } }
        }
    }

    func isOpen(_ id: String) -> Bool { sessions[id] != nil }

    func toggle(_ container: Container) {
        if isOpen(container.id) { close(container.id) } else { open(container) }
    }

    func open(_ container: Container) {
        guard sessions[container.id] == nil else { return }
        sessions[container.id] = TerminalSession(container: container)
    }

    func restart(_ container: Container) {
        close(container.id)
        open(container)
    }

    func close(_ id: String) {
        sessions[id]?.terminate()
        sessions[id] = nil
    }

    func closeAll() {
        for id in sessions.keys { close(id) }
    }
}

/// One shell: our own `kuvo shell` running under a real PTY.
@MainActor @Observable
final class TerminalSession: NSObject, @preconcurrency LocalProcessTerminalViewDelegate {
    let view = LocalProcessTerminalView(frame: .zero)
    private(set) var title = ""
    private(set) var ended = false

    init(container: Container) {
        super.init()
        view.processDelegate = self
        NSApp.effectiveAppearance.performAsCurrentDrawingAppearance { Self.style(view) }
        view.startProcess(
            executable: CLIInstaller.executable,
            args: ["shell", container.id],
            // The app is already the engine; its shells must never try to launch it again.
            environment: Terminal.getEnvironmentVariables(termName: "xterm-256color") + ["KUVO_NO_LAUNCH=1"]
        )
    }

    func terminate() { if !ended { view.terminate() } }

    func restyle() {
        NSApp.effectiveAppearance.performAsCurrentDrawingAppearance { Self.style(view) }
        view.needsDisplay = true
    }

    func sizeChanged(source: LocalProcessTerminalView, newCols: Int, newRows: Int) {}
    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}

    func setTerminalTitle(source: LocalProcessTerminalView, title: String) {
        self.title = title
    }

    func processTerminated(source: TerminalView, exitCode: Int32?) {
        withAnimation(.snappy) { ended = true }
    }

    /// Transparent background so the pane's glass shows through; system colors for ANSI.
    private static func style(_ view: LocalProcessTerminalView) {
        view.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        view.nativeForegroundColor = .labelColor
        view.nativeBackgroundColor = .clear
        view.wantsLayer = true
        view.layer?.backgroundColor = NSColor.clear.cgColor // SwiftTerm only sets this at setup.
        view.caretColor = .controlAccentColor
        view.selectedTextBackgroundColor = .selectedTextBackgroundColor
        let palette: [NSColor] = [
            .secondaryLabelColor, .systemRed, .systemGreen, .systemOrange, .systemBlue, .systemPurple, .systemTeal, .labelColor,
            .tertiaryLabelColor, .systemRed, .systemGreen, .systemYellow, .systemBlue, .systemPink, .systemCyan, .labelColor,
        ]
        view.installColors(palette.map(\.terminalColor))
    }
}

private extension NSColor {
    var terminalColor: SwiftTerm.Color {
        let rgb = usingColorSpace(.sRGB) ?? self
        return SwiftTerm.Color(
            red: UInt16(rgb.redComponent * 65535), green: UInt16(rgb.greenComponent * 65535), blue: UInt16(rgb.blueComponent * 65535)
        )
    }
}

// MARK: - Terminal app

/// Opens Terminal with a shell inside a container, via our own CLI.
enum TerminalLauncher {
    static func open(_ container: Container) {
        let script = """
        #!/bin/sh
        clear
        exec '\(CLIInstaller.executable)' shell \(container.id)
        """
        let url = FileManager.default.temporaryDirectory.appending(path: "kuvo-\(container.name).command")
        try? script.write(to: url, atomically: true, encoding: .utf8)
        try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
        NSWorkspace.shared.open(url)
    }
}

// MARK: - Pane

enum PanePlacement: String { case trailing, bottom }

/// A terminal as a tiled glass pane: header with title and controls, then the shell.
struct TerminalPane: View {
    let container: Container
    let session: TerminalSession
    @Binding var placement: PanePlacement
    @Binding var maximized: Bool
    let onRestart: () -> Void
    let onClose: () -> Void

    var body: some View {
        Pane(icon: "terminal") {
            HStack(spacing: 8) {
                Text(container.name)
                if !session.title.isEmpty {
                    Text(session.title).fontWeight(.regular).foregroundStyle(.tertiary)
                }
            }
        } accessory: {
            PaneButton(placement == .trailing ? "rectangle.split.1x2" : "rectangle.split.2x1",
                       help: placement == .trailing ? "Move below" : "Move beside") {
                placement = placement == .trailing ? .bottom : .trailing
            }
            PaneButton(maximized ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right",
                       help: maximized ? "Restore" : "Maximize") { maximized.toggle() }
            PaneButton("xmark", help: "Close terminal", action: onClose)
        } content: {
            TerminalHost(view: session.view)
                .padding(.leading, 14)
                .padding(.trailing, 6)
                .padding(.bottom, 10)
                .overlay {
                    if session.ended {
                        VStack(spacing: 10) {
                            Text("Session ended").font(.system(size: 13, weight: .medium))
                            Button("Start New Session", action: onRestart)
                        }
                        .padding(20)
                        .glassCard(cornerRadius: 12)
                        .transition(.scale(0.95).combined(with: .opacity))
                    }
                }
        }
    }
}

/// Borderless header control, like the reference's split / maximize / close.
struct PaneButton: View {
    let symbol: String
    let help: String
    let action: () -> Void
    @State private var hovering = false

    init(_ symbol: String, help: String, action: @escaping () -> Void) {
        self.symbol = symbol
        self.help = help
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 11.5))
                .frame(width: 24, height: 22)
                .contentShape(.rect)
                .foregroundStyle(hovering ? .primary : .secondary)
                .background(.primary.opacity(hovering ? 0.08 : 0), in: .rect(cornerRadius: 6))
                .contentTransition(.symbolEffect(.replace))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(help)
        .animation(.easeOut(duration: 0.12), value: hovering)
    }
}

/// Hosts a session's long-lived terminal view, moving it between containers
/// as SwiftUI recreates the pane, and focusing it when shown.
private struct TerminalHost: NSViewRepresentable {
    let view: LocalProcessTerminalView

    func makeNSView(context: Context) -> NSView {
        let host = FocusingHost()
        host.terminal = view
        attach(to: host)
        return host
    }

    func updateNSView(_ host: NSView, context: Context) {
        if view.superview !== host { attach(to: host) }
    }

    private func attach(to host: NSView) {
        view.removeFromSuperview()
        view.translatesAutoresizingMaskIntoConstraints = false
        host.addSubview(view)
        NSLayoutConstraint.activate([
            view.leadingAnchor.constraint(equalTo: host.leadingAnchor),
            view.trailingAnchor.constraint(equalTo: host.trailingAnchor),
            view.topAnchor.constraint(equalTo: host.topAnchor),
            view.bottomAnchor.constraint(equalTo: host.bottomAnchor),
        ])
    }
}

/// Gives the terminal keyboard focus as soon as it's in a window.
private final class FocusingHost: NSView {
    weak var terminal: NSView?

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let terminal { window?.makeFirstResponder(terminal) }
    }
}

// MARK: - Split

/// Two panes with a draggable gap between them; the second can be maximized.
struct PaneSplit<First: View, Second: View>: View {
    let placement: PanePlacement
    @Binding var fraction: Double
    let showsSecond: Bool
    let maximized: Bool
    @ViewBuilder var first: First
    @ViewBuilder var second: Second

    private let gap: CGFloat = 10
    /// The split position when the current drag began; drags are measured from it.
    @State private var dragOrigin: Double?

    var body: some View {
        GeometryReader { geometry in
            let horizontal = placement == .trailing
            let total = horizontal ? geometry.size.width : geometry.size.height
            let firstLength = showsSecond ? (maximized ? 0 : (total - gap) * fraction) : total
            let layout = horizontal ? AnyLayout(HStackLayout(spacing: 0)) : AnyLayout(VStackLayout(spacing: 0))

            layout {
                first
                    .frame(width: horizontal ? firstLength : nil, height: horizontal ? nil : firstLength)
                    .opacity(maximized && showsSecond ? 0 : 1)
                if showsSecond {
                    if !maximized { handle(horizontal: horizontal, total: total) }
                    let secondLength = maximized ? total : max(total - gap - firstLength, 0)
                    second
                        .frame(width: horizontal ? secondLength : nil, height: horizontal ? nil : secondLength)
                        .transition(.move(edge: horizontal ? .trailing : .bottom).combined(with: .opacity))
                }
            }
        }
        .animation(.smooth(duration: 0.35), value: showsSecond)
        .animation(.smooth(duration: 0.35), value: maximized)
        .animation(.smooth(duration: 0.35), value: placement)
    }

    private func handle(horizontal: Bool, total: CGFloat) -> some View {
        Color.clear
            .frame(width: horizontal ? gap : nil, height: horizontal ? nil : gap)
            .contentShape(.rect)
            .pointerStyle(horizontal ? .columnResize : .rowResize)
            .gesture(
                DragGesture(coordinateSpace: .global)
                    .onChanged { value in
                        // Translation is cumulative, so offset it from where the drag began,
                        // not from the already-moved position, or the divider runs ahead.
                        let origin = dragOrigin ?? fraction
                        dragOrigin = origin
                        let delta = horizontal ? value.translation.width : value.translation.height
                        fraction = min(max(origin + delta / (total - gap), 0.3), 0.75)
                    }
                    .onEnded { _ in dragOrigin = nil }
            )
    }
}

