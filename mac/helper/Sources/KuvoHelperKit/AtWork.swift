#if os(macOS)
import Foundation

/// What's at work in Kuvo on this Mac, as its box says it (`kuvo/box/at-work`, PROTOCOL.md "In the
/// menu bar"): facts only, which the menu bar item draws as they are. Nothing here decides anything
/// the box hasn't said.
public struct AtWork: Decodable, Equatable, Sendable {
    public struct Session: Decodable, Equatable, Sendable {
        public var id: String
        public var title: String?
        public var bot: String?
    }

    public struct Asking: Decodable, Equatable, Sendable {
        public var id: String
        public var title: String?
        public var asks: String?
    }

    public struct Waiting: Decodable, Equatable, Sendable {
        public var sessions: Int
        public var bots: Int
        public var queue: Int
    }

    public struct Ring: Decodable, Equatable, Sendable {
        public var used: Double
        public var level: String
    }

    public struct Account: Decodable, Equatable, Sendable {
        public var service: String
        public var title: String
        public var short: Ring?
        public var long: Ring?
    }

    public var applies: Bool
    public var working: Int
    public var sessions: [Session]
    public var asking: [Asking]
    public var waiting: Waiting
    public var background: Int
    public var busy: Bool
    public var rings: [Account]

    /// One line of the box's answer; nil for one this can't read (a newer box's may add fields,
    /// which are left as they are).
    public static func read(_ line: Data) -> AtWork? {
        try? JSONDecoder().decode(AtWork.self, from: line)
    }
}

/// What the menu bar item shows, from where Kuvo on this Mac is and what the box last said.
public struct MenuBarLook: Equatable, Sendable {
    /// Where Kuvo on this Mac is, as the helper sees it.
    public enum Stage: Equatable, Sendable {
        /// Its server hasn't answered yet (as it starts, or starts again).
        case starting
        case running(AtWork)
        /// Stop was asked for: it goes in a moment, the item with it.
        case stopping
    }

    public var stage: Stage

    public init(_ stage: Stage) {
        self.stage = stage
    }

    var atWork: AtWork? {
        if case .running(let atWork) = stage { return atWork }
        return nil
    }

    /// The count beside the mark: sessions at work, none while there are none.
    public var count: Int? {
        guard let working = atWork?.working, working > 0 else { return nil }
        return working
    }

    /// Whether the mark has its dot: something waits on you, in any area the box counts.
    public var needsYou: Bool {
        guard let waiting = atWork?.waiting else { return false }
        return waiting.sessions + waiting.bots + waiting.queue > 0
    }

    /// What VoiceOver says of the item.
    public var spoken: String {
        switch stage {
        case .starting: return "Kuvo, starting"
        case .stopping: return "Kuvo, stopping"
        case .running(let atWork):
            var said = ["Kuvo"]
            if atWork.working > 0 { said.append(atWork.working == 1 ? "1 session at work" : "\(atWork.working) sessions at work") }
            if atWork.waiting.sessions > 0 { said.append("\(atWork.waiting.sessions) waiting on you") }
            if said.count == 1 { said.append("nothing at work") }
            return said.joined(separator: ", ")
        }
    }

    /// The panel's line beside its title.
    public var status: String {
        switch stage {
        case .starting: return "Starting…"
        case .stopping: return "Stopping…"
        case .running: return "Running"
        }
    }

    /// Whether Stop asks first: while the box says something would stop, or while that isn't known,
    /// or while the desktop app is open (`appOpen`: its window closes too).
    public func stopAsks(appOpen: Bool) -> Bool { appOpen || (atWork?.busy ?? true) }

    /// The question Stop asks first, in words.
    public func stopQuestion(appOpen: Bool) -> (title: String, detail: String) {
        let working = atWork?.working ?? 0
        var said: [String] = []
        switch working {
        case 0 where atWork == nil: said.append("Kuvo may be at work.")
        case 0 where atWork?.busy == true: said.append("Work is still running.")
        case 0: break
        case 1: said.append("A session is at work.")
        default: said.append("\(working) sessions are at work.")
        }
        said.append(appOpen ? "Kuvo stops now, and its window closes. It starts again when you open Kuvo." : "Kuvo stops now, and starts again when you open it.")
        return ("Stop Kuvo on this Mac?", said.joined(separator: " "))
    }

    /// Whether Stop can be asked for now.
    public var canStop: Bool { stage != .stopping }
}
#endif
