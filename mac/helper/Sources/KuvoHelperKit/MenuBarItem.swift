#if os(macOS)
import AppKit
import os
import SwiftUI

nonisolated private let log = Logger(subsystem: "run.kuvo.code", category: "menu-bar")

/// Kuvo in the menu bar (PROTOCOL.md, "In the menu bar"), drawn by Kuvo Helper for as long as it
/// runs Kuvo on this Mac: it's the part that always runs, so the item is there whenever Kuvo is,
/// with the app open or not, and goes with it. What it shows is the box's (`AtWorkFeed`): every one
/// of your computers, as this Mac's Kuvo hears of them; it offers Open Kuvo (the desktop app, opened
/// or brought forward, at one of your computers or a session there) and Stop Kuvo on this Mac (asked first
/// while the box says something would stop), which the box does (`kuvo/box/stop`): the helper goes
/// once its keeper has, the item with it.
@MainActor
public final class MenuBarItem: NSObject {
    /// The one this helper draws (kept for as long as it runs).
    public static var shared: MenuBarItem?

    private let item: NSStatusItem
    private let popover = NSPopover()
    private let app: String
    private var feed: AtWorkFeed!
    private(set) var look = MenuBarLook(.starting) {
        didSet { if look != oldValue { drawn() } }
    }

    /// `helper`: whose box it shows; `app`: the desktop app's bundle ID, which Open Kuvo opens.
    public init(helper: BoxHelper, app: String) {
        self.app = app
        item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        super.init()
        let run = helper.run
        feed = AtWorkFeed(server: { [weak helper] in BoxServer.recorded(in: run, password: helper?.servePassword) }) { [weak self] atWork in
            guard let self, self.look.stage != .stopping else { return }
            self.look = MenuBarLook(atWork.map { .running($0) } ?? .starting)
        }
        popover.behavior = .transient
        popover.animates = true
        item.button?.target = self
        item.button?.action = #selector(toggle)
        item.button?.imagePosition = .imageLeading
        drawn()
        feed.start()
    }

    /// The item as the look says: the mark, the count of sessions at work beside it, and a dot
    /// while something waits on you.
    private func drawn() {
        guard let button = item.button else { return }
        button.image = MenuBarMark.image()
        let title = NSMutableAttributedString()
        if let count = look.count {
            title.append(NSAttributedString(string: " \(count)", attributes: [.font: NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .medium)]))
        }
        if look.needsYou {
            title.append(NSAttributedString(string: " ●", attributes: [.font: NSFont.systemFont(ofSize: 7), .foregroundColor: NSColor.systemOrange, .baselineOffset: 3]))
        }
        button.attributedTitle = title
        button.setAccessibilityLabel(look.spoken)
        button.toolTip = look.spoken
        if popover.isShown { panelDrawn() }
    }

    private func panelDrawn() {
        let panel = MenuBarPanel(look: look, actions: MenuBarActions(open: { [weak self] at in self?.openKuvo(at) }, stop: { [weak self] in self?.stopKuvo() }))
        if let hosting = popover.contentViewController as? NSHostingController<MenuBarPanel> {
            hosting.rootView = panel
        } else {
            let hosting = NSHostingController(rootView: panel)
            hosting.sizingOptions = [.preferredContentSize]
            popover.contentViewController = hosting
        }
    }

    @objc private func toggle() {
        if popover.isShown { return popover.performClose(nil) }
        guard let button = item.button else { return }
        panelDrawn()
        NSApp.activate()
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        popover.contentViewController?.view.window?.makeKey()
    }

    /// Open Kuvo: the desktop app, opened where it's closed, brought forward where it's open; at
    /// `at` (one of your computers, a session there) where it's given. The very copy that opened with
    /// this Mac's Kuvo last, as it told the box, else the newest copy installed (`DesktopApp`).
    func openKuvo(_ at: OpenAt? = nil) {
        popover.performClose(nil)
        Task {
            let registered = await feed.registeredDesktop()
            let app = self.app
            let copies = NSWorkspace.shared.urlsForApplications(withBundleIdentifier: app)
            guard let url = DesktopApp.pick(registered: registered, copies: copies, app: app, trusted: { BoxHelperBundle.isKuvoCode($0, identifier: app) }) else {
                return say("Kuvo isn’t installed on this Mac", "Install Kuvo again to open it. What’s at work here goes on meanwhile.")
            }
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = true
            let opened: @Sendable (Error?) -> Void = { error in
                guard let error else { return }
                log.error("Kuvo didn't open: \(error.localizedDescription, privacy: .public)")
                let why = error.localizedDescription
                Task { @MainActor [weak self] in self?.say("Kuvo didn’t open", why) }
            }
            if let at {
                NSWorkspace.shared.open([at.url], withApplicationAt: url, configuration: configuration) { _, error in opened(error) }
            } else {
                NSWorkspace.shared.openApplication(at: url, configuration: configuration) { _, error in opened(error) }
            }
        }
    }

    /// The desktop app, where it's open.
    private var openApps: [NSRunningApplication] { NSRunningApplication.runningApplications(withBundleIdentifier: app) }

    /// How long a stop that was taken has to end the helper before the item says it didn't.
    static let stopsWithin: Duration = .seconds(30)

    /// Stop Kuvo on this Mac: asked first while the box says something would stop (or doesn't
    /// say yet), or while the desktop app is open, whose window closes too (with nothing to reach,
    /// as 0.7's Stop Background Work quit it); then asked of the box.
    func stopKuvo() {
        popover.performClose(nil)
        guard look.canStop else { return }
        let appOpen = !openApps.isEmpty
        if look.stopAsks(appOpen: appOpen) {
            NSApp.activate()
            let alert = NSAlert()
            let question = look.stopQuestion(appOpen: appOpen)
            alert.messageText = question.title
            alert.informativeText = question.detail
            let stop = alert.addButton(withTitle: "Stop")
            stop.hasDestructiveAction = true
            alert.addButton(withTitle: "Cancel")
            guard alert.runModal() == .alertFirstButtonReturn else { return }
        }
        let before = look
        look = MenuBarLook(.stopping)
        Task {
            if let why = await feed.askToStop() {
                look = before
                return say("Kuvo on this Mac didn’t stop", why)
            }
            // (Its window would only say Kuvo isn't answering.)
            for open in openApps { open.terminate() }
            // This helper goes once its keeper has; still here a while on, the stop didn't take.
            try? await Task.sleep(for: Self.stopsWithin)
            guard look.stage == .stopping else { return }
            look = MenuBarLook(.starting)
            say("Kuvo on this Mac didn’t stop", "It went on running. Try again.")
        }
    }

    private func say(_ title: String, _ detail: String) {
        NSApp.activate()
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = detail
        alert.runModal()
    }

}
#endif
