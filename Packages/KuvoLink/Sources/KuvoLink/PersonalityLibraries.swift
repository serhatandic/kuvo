import Foundation

/// The personalities you make are one library on every computer: each keeps its copy, and copies
/// meet as a Mac connects to a machine, or as one changes them (PROTOCOL.md, "Personalities"). Per
/// personality, the newest change wins; a removal over a change at the same moment, then the words
/// that sort last, so every copy picks the same; a removal stays noted, so no copy brings it back.
/// The same rules as the bridge's `mergePersonalities`, held to `Vectors/personalities.json`.
public enum PersonalityLibraries {
    /// One you made, as a copy keeps it.
    public struct Kept: Codable, Hashable, Sendable {
        public var id: String
        public var name: String
        public var description: String
        public var symbol: String
        /// When it was made or last changed, in milliseconds since 1970.
        public var updated: Double

        public init(id: String, name: String, description: String, symbol: String, updated: Double) {
            self.id = id
            self.name = name
            self.description = description
            self.symbol = symbol
            self.updated = updated
        }
    }

    /// One removed, and when.
    public struct Removal: Codable, Hashable, Sendable {
        public var id: String
        public var removed: Double

        public init(id: String, removed: Double) {
            self.id = id
            self.removed = removed
        }
    }

    public struct Library: Codable, Equatable, Sendable {
        public var personalities: [Kept]
        public var removed: [Removal]

        public init(personalities: [Kept] = [], removed: [Removal] = []) {
            self.personalities = personalities
            self.removed = removed
        }
    }

    /// Removals noted, the newest kept.
    public static let mostRemovals = 500

    /// Libraries as one, in the order their personalities were first seen. Kuvo Code's presets,
    /// and one without a name or words, aren't in it.
    public static func merged(_ libraries: [Library]) -> Library {
        enum Take { case kept(Kept), gone(Removal) }
        func time(_ take: Take) -> Double {
            switch take {
            case .kept(let kept): kept.updated
            case .gone(let removal): removal.removed
            }
        }
        func newer(_ a: Take, _ b: Take) -> Take {
            if time(a) != time(b) { return time(a) > time(b) ? a : b }
            switch (a, b) {
            case (.gone, _): return a
            case (_, .gone): return b
            case (.kept(let x), .kept(let y)):
                // As the bridge compares them: by UTF-16 code unit.
                return (x.name + "\n" + x.description).utf16.lexicographicallyPrecedes((y.name + "\n" + y.description).utf16) ? b : a
            }
        }
        var order: [String] = []
        var byID: [String: Take] = [:]
        func meet(_ take: Take, id: String) {
            guard id.hasPrefix("custom."), id.count > "custom.".count else { return }
            if let had = byID[id] {
                byID[id] = newer(had, take)
            } else {
                order.append(id)
                byID[id] = take
            }
        }
        for library in libraries {
            for kept in library.personalities where !kept.name.isEmpty && !kept.description.isEmpty { meet(.kept(kept), id: kept.id) }
            for removal in library.removed { meet(.gone(removal), id: removal.id) }
        }
        var met = Library()
        for id in order {
            switch byID[id] {
            case .kept(let kept)?: met.personalities.append(kept)
            case .gone(let removal)?: met.removed.append(removal)
            case nil: break
            }
        }
        met.removed = Array(met.removed.sorted { $0.removed > $1.removed }.prefix(mostRemovals))
        return met
    }
}
