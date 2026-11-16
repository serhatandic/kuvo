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
    /// Every one of your computers, this one first (`computers`, #781); none from an older box.
    public var computers: [Computer]?

    /// One of your computers as the box says it: its key and name, whether it's this one, whether
    /// it's heard from, what's at work there and what waits on you (counted where it said), and
    /// the rings of the accounts signed in there.
    public struct Computer: Decodable, Equatable, Sendable {
        public var key: String
        public var name: String
        public var kind: String
        public var here: Bool?
        public var reach: String
        public var working: Int?
        public var sessions: [Session]
        public var waiting: Int?
        public var asking: [Asking]
        public var rings: [Account]

        public var isHere: Bool { here == true }
    }

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

    /// Your computers, as the box says them, this one first; from an older box that doesn't list
    /// them, this one alone, from what it says of itself.
    public var computers: [AtWork.Computer] {
        guard let atWork else { return [] }
        if let computers = atWork.computers, !computers.isEmpty { return computers }
        return [AtWork.Computer(key: "", name: "This Mac", kind: "mac", here: true, reach: "connected", working: atWork.working, sessions: atWork.sessions,
                                waiting: atWork.waiting.sessions, asking: atWork.asking, rings: atWork.rings)]
    }

    /// How many sessions are at work on all your computers.
    var workingEverywhere: Int { computers.reduce(0) { $0 + ($1.working ?? 0) } }

    /// How many sessions wait on you on your other computers (this one's are in `waiting`).
    var waitingElsewhere: Int { computers.filter { !$0.isHere }.reduce(0) { $0 + max($1.waiting ?? 0, $1.asking.count) } }

    /// The count beside the mark: sessions at work on all your computers, none while there are none.
    public var count: Int? {
        let working = workingEverywhere
        return working > 0 ? working : nil
    }

    /// Whether the mark has its dot: something waits on you, in any area this box counts, or a
    /// session on another of your computers.
    public var needsYou: Bool {
        guard let waiting = atWork?.waiting else { return false }
        return waiting.sessions + waiting.bots + waiting.queue + waitingElsewhere > 0
    }

    /// What VoiceOver says of the item.
    public var spoken: String {
        switch stage {
        case .starting: return "Kuvo, starting"
        case .stopping: return "Kuvo, stopping"
        case .running(let atWork):
            var said = ["Kuvo"]
            let working = workingEverywhere, waiting = atWork.waiting.sessions + waitingElsewhere
            if working > 0 { said.append(working == 1 ? "1 session at work" : "\(working) sessions at work") }
            if waiting > 0 { said.append("\(waiting) waiting on you") }
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
