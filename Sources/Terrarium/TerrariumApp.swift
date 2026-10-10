import SwiftUI

/// One binary, two roles: the app, or `Terrarium mcp` for agents.
@main
enum Entry {
    static func main() {
        guard CommandLine.arguments.dropFirst().first == "mcp" else { return TerrariumApp.main() }
        Task {
            await MCPServer.run()
            exit(0)
        }
        dispatchMain()
    }
}

struct TerrariumApp: App {
    @NSApplicationDelegateAdaptor private var delegate: AppDelegate

    var body: some Scene {
        Window("Terrarium", id: "main") {
            ContentView()
                .environment(delegate.store)
                .environment(delegate.engine)
        }
        .defaultSize(width: 1080, height: 680)
        .windowToolbarStyle(.unified)

        MenuBarExtra {
            MenuBarView()
                .environment(delegate.store)
                .environment(delegate.engine)
        } label: {
            Image(systemName: delegate.store.containers.contains(where: \.isRunning) ? "shippingbox.fill" : "shippingbox")
        }
        .menuBarExtraStyle(.window)
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let engine = Engine()
    let store = DockerStore()

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Lets `swift run` behave like a real app (dock icon, focus) without a bundle.
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()

        store.socketPath = { [engine] in engine.socketPath }
        store.onPortsChanged = { [engine] ports in engine.forward(ports) }
        Task { await engine.start() }
        Task { await store.run() }
        #if DEBUG
        startSnapshots()
        #endif
    }

    /// Give dockerd a chance to shut down cleanly before we exit.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard engine.state != .stopped else { return .terminateNow }
        Task {
            await engine.stop()
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }

    #if DEBUG
    /// `TERRARIUM_SNAPSHOT=/tmp/shot.png swift run` writes the main window to disk
    /// every two seconds. Captures only our own window, no screen recording needed.
    private func startSnapshots() {
        guard let path = ProcessInfo.processInfo.environment["TERRARIUM_SNAPSHOT"] else { return }
        Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
            MainActor.assumeIsolated {
                guard let view = NSApp.windows.first(where: { $0.isVisible && $0.identifier?.rawValue.hasPrefix("main") == true })?.contentView?.superview,
                      let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
                view.cacheDisplay(in: view.bounds, to: rep)
                try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
            }
        }
    }
    #endif
}
