import Foundation
import Sparkle

/// Software updates, via Sparkle. Release builds name an update feed in their
/// Info.plist (see scripts/build-app.sh); development builds have none, so the
/// updater never starts and the update controls stay hidden.
@MainActor @Observable
final class Updates {
    /// Whether this build can update itself at all.
    let isAvailable: Bool
    /// False while a check is already running.
    private(set) var canCheck = false
    /// Check once a day in the background.
    var checksAutomatically: Bool {
        didSet { controller?.updater.automaticallyChecksForUpdates = checksAutomatically }
    }

    @ObservationIgnored private let controller: SPUStandardUpdaterController?
    @ObservationIgnored private var observation: NSKeyValueObservation?

    init() {
        guard Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") != nil else {
            isAvailable = false
            checksAutomatically = false
            controller = nil
            return
        }
        let controller = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: nil, userDriverDelegate: nil)
        self.controller = controller
        isAvailable = true
        checksAutomatically = controller.updater.automaticallyChecksForUpdates
        observation = controller.updater.observe(\.canCheckForUpdates, options: [.initial, .new]) { [weak self] _, change in
            let canCheck = change.newValue ?? false
            MainActor.assumeIsolated { self?.canCheck = canCheck }
        }
    }

    func check() {
        controller?.checkForUpdates(nil)
    }
}
