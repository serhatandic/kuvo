import Foundation

// MARK: Settings › Permissions' words

/// How both apps show a computer's sandbox atop its Permissions page: when it's on, one quiet line;
/// when it needs you, an orange row with why, in the box's own sentence (`why`), and the one button
/// that fixes it, in the box's words (`actionLabel`). No client keeps words for a reason.
public struct SandboxNotice: Equatable, Sendable {
    public enum Look: Sendable {
        /// On: a small green shield and the line, in secondary ink.
        case on
        /// Being checked: a small loader and the line.
        case checking
        /// Commands run outside the sandbox, or not at all, and it needs you: orange.
        case needsYou
    }

    /// What a button does, by the box's word for it (`SandboxState.action`); Report a Problem is the
    /// client's own.
    public enum Action: Equatable, Sendable {
        /// Checks it again (`kuvo/sandbox/check`), which also puts back what was changed.
        case check
        /// Starts the computer's engine over; `asking` first when what agents are doing stops.
        case restartEngine(asking: Bool)
        /// The docs on what a machine needs.
        case help
        /// macOS's Software Update.
        case softwareUpdate
        /// Report a Problem, about the check that keeps failing.
        case report
    }

    public struct Button: Equatable, Sendable {
        public var title: String
        public var action: Action
        /// The page it opens, as the box names it (`opens`), where its action sends no route.
        public var opens: URL?
        /// That address is the computer's own (its Software Update pane): only a client running on
        /// that computer draws the button.
        public var opensHere: Bool

        public init(title: String, action: Action, opens: URL? = nil, opensHere: Bool = false) {
            self.title = title
            self.action = action
            self.opens = opens
            self.opensHere = opensHere
        }
    }

    /// The docs on what a machine needs for its sandbox, for a machine whose sandbox is on but
    /// can't keep commands from your login's services (the box names no page for that yet).
    public static let machinesHelp = URL(string: "https://www.kuvo.run/code/docs/machines/#sandbox")

    public var look: Look
    /// The row's title; none for a one-line state.
    public var title: String?
    public var detail: String
    public var buttons: [Button]
}

extension SandboxState {
    /// Its line or row, as the box says it is now.
    public var notice: SandboxNotice {
        if on, gaps?.contains(.services) == true {
            // On, but not all of it: systemd can run what a command asks outside it.
            let fix = needs == "util-linux" ? "has util-linux" : "allows user namespaces"
            return SandboxNotice(look: .needsYou, title: "Commands can still reach your login’s services",
                                 detail: "They run in the sandbox, but this machine doesn’t let it keep them from services like systemd. Until it \(fix), agents don’t use your vault here and goals don’t run on their own.",
                                 buttons: [.init(title: "How to Check…", action: .help, opens: SandboxNotice.machinesHelp)])
        }
        if on { return SandboxNotice(look: .on, title: nil, detail: "Agents’ commands run in a sandbox: they can’t read your keys or sign-ins.", buttons: []) }
        // Not on: the box's sentence as it is, and its button. Being checked is a quiet line.
        let detail = why ?? "Its computer didn’t say why."
        if reason == .checking { return SandboxNotice(look: .checking, title: nil, detail: why ?? "Checking the sandbox…", buttons: []) }
        // (Report a Problem, the client's own, once its check has failed more than once.)
        let report: [SandboxNotice.Button] = (failures ?? 0) > 1 ? [.init(title: "Report a Problem…", action: .report)] : []
        return SandboxNotice(look: .needsYou, title: "The sandbox isn’t on", detail: detail, buttons: (button.map { [$0] } ?? []) + report)
    }

    /// Its button while it isn't on: the box's words for it (`actionLabel`), and what it does by
    /// the box's word (`action`): its route (the check again; the engine restarted, asking first
    /// where its words end in an ellipsis), or the page it opens (`opens`: a web page, unless the
    /// address is that computer's own, `opensHere`). None where the box names no words, or
    /// nothing a client can do.
    private var button: SandboxNotice.Button? {
        guard let actionLabel, !actionLabel.isEmpty else { return nil }
        switch action {
        case .retry?, .fix?: return .init(title: actionLabel, action: .check)
        case .restartEngine?: return .init(title: actionLabel, action: .restartEngine(asking: actionLabel.hasSuffix("…")))
        case .help?, .softwareUpdate?:
            let here = opensHere == true
            guard let page = opens.flatMap(URL.init(string:)), here || ["https", "http"].contains(page.scheme?.lowercased() ?? "") else { return nil }
            return .init(title: actionLabel, action: action == .help ? .help : .softwareUpdate, opens: page, opensHere: here)
        case nil:
            // An action a newer box names: its words, where it sends the check's route.
            guard let route, route.hasSuffix(SandboxRoutes.check) else { return nil }
            return .init(title: actionLabel, action: .check)
        }
    }
}
