import SwiftUI

/// One binary, two roles: the app, or the `kuvo` command-line tool.
@main
enum Entry {
    static func main() {
        // Writing to a socket the other side already closed must be an error we
        // handle, not a SIGPIPE that silently kills the app (or the CLI).
        signal(SIGPIPE, SIG_IGN)
        let arguments = Array(CommandLine.arguments.dropFirst())
        let invokedAsTool = (CommandLine.arguments.first as NSString?)?.lastPathComponent == "kuvo"
        let looksLikeCommand = arguments.first.map { !$0.hasPrefix("-") || ["-h", "--help", "--version"].contains($0) } ?? false
        guard invokedAsTool || looksLikeCommand else { return KuvoApp.main() }
        Task {
            await CLI.main(arguments)
            exit(0)
        }
        dispatchMain()
    }
}

struct KuvoApp: App {
    @NSApplicationDelegateAdaptor private var delegate: AppDelegate
    @AppStorage(Appearance.key) private var appearance = Appearance.system

    var body: some Scene {
        Window("Kuvo", id: "main") {
            ContentView()
                .environment(delegate.store)
                .environment(delegate.engine)
                .environment(delegate.terminals)
        }
        .defaultSize(width: 1080, height: 680)
        .windowToolbarStyle(.unified)
        .commands {
            CommandGroup(replacing: .appInfo) {
                Button("About Kuvo") { showAbout(engine: delegate.store.connection) }
            }
            CommandGroup(after: .sidebar) {
                Picker("Appearance", selection: $appearance) {
                    ForEach(Appearance.allCases, id: \.self) { Text($0.title).tag($0) }
                }
            }
        }

        MenuBarExtra {
            MenuBarView()
                .environment(delegate.store)
                .environment(delegate.engine)
                .environment(delegate.terminals)
        } label: {
            Image(systemName: delegate.store.containers.contains(where: \.isRunning) ? "shippingbox.fill" : "shippingbox")
        }
        .menuBarExtraStyle(.window)

        Settings { SettingsView() }
    }
}

/// The standard About panel, with the Docker engine's version as its credits.
@MainActor private func showAbout(engine connection: DockerStore.Connection) {
    let engine = if case .connected(let name) = connection { "Runs \(name)" } else { "Docker engine not running" }
    let style = NSMutableParagraphStyle()
    style.alignment = .center
    NSApp.orderFrontStandardAboutPanel(options: [
        .credits: NSAttributedString(string: engine, attributes: [
            .font: NSFont.systemFont(ofSize: 11),
            .foregroundColor: NSColor.secondaryLabelColor,
            .paragraphStyle: style,
        ]),
    ])
    NSApp.activate()
}

/// System, light or dark, applied to the whole app (AppKit views and the menu bar panel included).
enum Appearance: String, CaseIterable {
    case system, light, dark
    static let key = "appearance"

    var title: String {
        switch self {
        case .system: "System"
        case .light: "Light"
        case .dark: "Dark"
        }
    }

    @MainActor func apply() {
        NSApp.appearance = switch self {
        case .system: nil
        case .light: NSAppearance(named: .aqua)
        case .dark: NSAppearance(named: .darkAqua)
        }
    }
}

private struct SettingsView: View {
    @AppStorage(ArchiveRetention.key, store: ArchiveRetention.defaults) private var retentionDays = 7
    @AppStorage(Appearance.key) private var appearance = Appearance.system

    var body: some View {
        Form {
            Picker("Appearance", selection: $appearance) {
                ForEach(Appearance.allCases, id: \.self) { Text($0.title).tag($0) }
            }
            .pickerStyle(.segmented)
            Picker("Delete archived containers after", selection: $retentionDays) {
                ForEach(ArchiveRetention.choices, id: \.self) { days in
                    Text(days == 0 ? "Never" : days == 1 ? "1 day" : "\(days) days").tag(days)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 440)
        .fixedSize()
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let engine = Engine()
    let store = DockerStore()
    let terminals = Terminals()

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Lets `swift run` behave like a real app (dock icon, focus) without a bundle.
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()

        store.socketPath = { [engine] in engine.socketPath }
        store.onPortsChanged = { [engine] ports in engine.forward(ports) }
        Task { await engine.start() }
        Task { await store.run() }
        Task.detached(priority: .background) { CLIInstaller.ensureInstalled() }
        #if DEBUG
        startSnapshots()
        #endif
    }

    /// Give dockerd a chance to shut down cleanly before we exit.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        terminals.closeAll()
        guard engine.state != .stopped else { return .terminateNow }
        Task {
            await engine.stop()
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }

    #if DEBUG
    /// `KUVO_SNAPSHOT=/tmp/shot.png swift run` writes the main window to disk
    /// every two seconds. Captures only our own window, no screen recording needed.
    private func startSnapshots() {
        guard let path = ProcessInfo.processInfo.environment["KUVO_SNAPSHOT"] else { return }
        Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
            MainActor.assumeIsolated {
                guard let window = NSApp.windows.first(where: { $0.isVisible && $0.identifier?.rawValue.hasPrefix("main") == true })
                else { return }
                // A real composited capture of our own window (materials included), falling back to drawing the views.
                if let image = Self.captureWindow(window.windowNumber) {
                    try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
                } else if let view = window.contentView?.superview, let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
                    view.cacheDisplay(in: view.bounds, to: rep)
                    try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
                }
            }
        }
    }

    /// CGWindowListCreateImage is hidden from Swift in newer SDKs; look it up at runtime.
    private static func captureWindow(_ number: Int) -> CGImage? {
        typealias Create = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
        guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else { return nil }
        let create = unsafeBitCast(symbol, to: Create.self)
        // Including just this window; ignore framing shadow, best resolution.
        return create(.null, 1 << 3, UInt32(number), (1 << 0) | (1 << 3))?.takeRetainedValue()
    }
    #endif
}
