import SwiftUI

@main
struct TerrariumApp: App {
    @NSApplicationDelegateAdaptor private var delegate: AppDelegate
    @State private var store = DockerStore()

    var body: some Scene {
        Window("Terrarium", id: "main") {
            ContentView()
                .environment(store)
                .task { await store.run() }
        }
        .defaultSize(width: 1080, height: 680)
        .windowToolbarStyle(.unified)

        MenuBarExtra {
            MenuBarView().environment(store)
        } label: {
            Image(systemName: store.containers.contains(where: \.isRunning) ? "shippingbox.fill" : "shippingbox")
        }
        .menuBarExtraStyle(.window)
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        // Lets `swift run` behave like a real app (dock icon, focus) without a bundle.
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
        #if DEBUG
        startSnapshots()
        #endif
    }

    #if DEBUG
    /// `TERRARIUM_SNAPSHOT=/tmp/shot.png swift run` writes the main window to disk
    /// every two seconds. Captures only our own window, no screen recording needed.
    private func startSnapshots() {
        guard let path = ProcessInfo.processInfo.environment["TERRARIUM_SNAPSHOT"] else { return }
        Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
            MainActor.assumeIsolated {
                guard let view = NSApp.windows.first(where: { $0.isVisible && $0.title == "Terrarium" })?.contentView?.superview,
                      let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
                view.cacheDisplay(in: view.bounds, to: rep)
                try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
            }
        }
    }
    #endif
}
