#if os(macOS)
import Foundation

/// Where Open Kuvo opens the desktop app: one of your computers (its key; this Mac's, none), and a
/// session there, as a `kuvo-code://open` link the desktop app takes (PROTOCOL.md, "In the menu bar").
public struct OpenAt: Equatable, Sendable {
    public var computer: String?
    public var session: String?

    public init(computer: String?, session: String? = nil) {
        self.computer = computer
        self.session = session
    }

    /// The link: `kuvo-code://open?computer=<key>&session=<id>`, each where there's one.
    public var url: URL {
        var parts = URLComponents()
        parts.scheme = "kuvo-code"
        parts.host = "open"
        var items: [URLQueryItem] = []
        if let computer, !computer.isEmpty { items.append(URLQueryItem(name: "computer", value: computer)) }
        if let session, !session.isEmpty { items.append(URLQueryItem(name: "session", value: session)) }
        parts.queryItems = items.isEmpty ? nil : items
        return parts.url!
    }
}

/// Which copy of Kuvo's desktop app Open Kuvo opens: the very one that opened with this Mac's Kuvo
/// last (it says where it is, `kuvo/box/app`'s `desktop`), where it's still there and is Kuvo;
/// else the newest of the copies with Kuvo's bundle ID, never an older one macOS might pick first.
public enum DesktopApp {
    /// What a copy of an app says of itself: its bundle ID, and its version and build.
    public struct About: Equatable, Sendable {
        public var id: String?
        public var version: String?
        public var build: String?

        public init(id: String?, version: String?, build: String?) {
            self.id = id
            self.version = version
            self.build = build
        }

        /// As its bundle's Info.plist says.
        public static func of(_ app: URL) -> About? {
            guard let bundle = Bundle(url: app) else { return nil }
            return About(id: bundle.bundleIdentifier, version: bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
                         build: bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String)
        }
    }

    /// The copy to open: `registered` where it's there (`about` reads it), has bundle ID `app` and
    /// `trusted` takes it for Kuvo; else the newest of `copies` with that ID (by version, then build).
    public static func pick(registered: URL?, copies: [URL], app: String, about: (URL) -> About? = About.of, trusted: (URL) -> Bool) -> URL? {
        if let registered, about(registered)?.id == app, trusted(registered) { return registered }
        let found = copies.compactMap { copy in about(copy).flatMap { $0.id == app ? (copy, $0) : nil } }
        return found.max { older, newer in isOlder(older.1, than: newer.1) }?.0
    }

    /// Whether `one` is an older version than `other`, as Sparkle orders Kuvo's builds
    /// (`CFBundleVersion`, scripts/build-version.sh: 0.7.0b1 < 0.7.0b2 < 0.7.0); by its version
    /// (`1.0.0-beta.3`) where it has no build.
    static func isOlder(_ one: About, than other: About) -> Bool {
        rank(one).lexicographicallyPrecedes(rank(other))
    }

    /// A version's numbers, then its beta's (a release's above every beta's).
    private static func rank(_ about: About) -> [Int] {
        let text = about.build ?? about.version ?? ""
        let main = text.prefix { $0.isNumber || $0 == "." }
        var numbers = main.split(separator: ".").compactMap { Int($0) }
        while numbers.count < 4 { numbers.append(0) }
        let rest = text.dropFirst(main.count)
        let beta = rest.isEmpty ? Int.max : rest.split(whereSeparator: { !$0.isNumber }).first.flatMap { Int($0) } ?? 0
        return Array(numbers.prefix(4)) + [beta]
    }
}
#endif
