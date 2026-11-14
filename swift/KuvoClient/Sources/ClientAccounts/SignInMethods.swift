import Foundation
import OpenCodeKit

/// The ways to sign in to an account, as its box offers them (`kuvo/sign-in/methods`, PROTOCOL.md
/// "Signing in"): in the order to draw them, each with the words its button says. The box decides
/// which, the order and the words; a client draws them as they come, the legacy ones apart, after
/// the others.
public struct SignInMethods: Decodable, Sendable, Equatable {
    /// One way in: the engine's own (`method`, as a client starts it with), and the box's words.
    public struct Offered: Decodable, Sendable, Hashable, Identifiable {
        public let method: Integration.Method
        public let label: String
        /// Kept by its maker for older setups: offered apart, after the others.
        public let legacy: Bool

        public var id: String { method.id }

        private enum CodingKeys: String, CodingKey { case label, legacy }

        public init(from decoder: Decoder) throws {
            method = try Integration.Method(from: decoder)
            let container = try decoder.container(keyedBy: CodingKeys.self)
            label = try container.decode(String.self, forKey: .label)
            legacy = try container.decodeIfPresent(Bool.self, forKey: .legacy) ?? false
        }
    }

    public let methods: [Offered]
    /// The environment variables that sign in to it too, set in the computer's shell.
    public let environment: [String]
    /// The way in its card names under its name ("Sign in with ChatGPT", "API key", "Connect").
    public let hint: String

    /// The ways to offer first, in order.
    public var primary: [Offered] { methods.filter { !$0.legacy } }
    /// Those offered apart, after them.
    public var legacy: [Offered] { methods.filter(\.legacy) }

    /// The feature that says a box answers it, and `SignInStatus` (`kuvo/info`).
    public static let feature = "sign-in.methods"
    public static let route = "kuvo/sign-in/methods"

    /// The route that asks it, for `integration`: a coding agent's account (`agents`) on the
    /// agents' bridge, else the engine's.
    public static func path(integration: String, agents: Bool) -> String {
        route + "?" + SignInQuery.encoded([("integration", integration)] + (agents ? [("agents", "1")] : []))
    }
}

/// How an account stands and what can be done with it, as its box says (`state`, `line`,
/// `connections` and `actions` in `kuvo/sign-in/accounts`; PROTOCOL.md, `sign-in.accounts.state`).
/// The box decides all of it; a client draws it, and keeps no rule for when a button shows.
public struct AccountSaid: Decodable, Sendable, Equatable {
    /// The fact its line words. One a newer box names that this client doesn't know reads as
    /// `signedOut`.
    public enum State: String, Sendable, Equatable {
        case signedIn, signedOut, ended, notInstalled, unchecked, problem

        /// Whether it needs you: its sign-in ended, or the one in use says something's wrong.
        public var needsYou: Bool { self == .ended || self == .problem }
    }

    /// What a client asks before it does an action: the question, what it comes to, and the words
    /// of the button that goes ahead.
    public struct Confirm: Decodable, Sendable, Equatable {
        public let title: String
        public let message: String
        public let label: String
    }

    /// Something that can be done with the account now.
    public struct Action: Decodable, Sendable, Equatable, Identifiable {
        /// `signIn`, `signInAgain`, `addAnother`, `disconnect`, `install`, `update`, `signOut`,
        /// `rename`, `remove`; a client leaves out one it has no control for.
        public let id: String
        /// Its button's words.
        public let label: String
        /// There where a client asks first, in the box's words. A client marks such a button as
        /// its platform does (an ellipsis); the label never carries one.
        public let confirm: Confirm?
        /// It only works on the box's computer (a sign-in whose page comes back to it): a client
        /// elsewhere says where instead of offering it.
        public let here: Bool

        private enum CodingKeys: String, CodingKey { case id, label, confirm, here }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            id = try container.decode(String.self, forKey: .id)
            label = try container.decode(String.self, forKey: .label)
            confirm = try container.decodeIfPresent(Confirm.self, forKey: .confirm)
            here = try container.decodeIfPresent(Bool.self, forKey: .here) ?? false
        }
    }

    /// One of its sign-ins or keys, in the engine's order (the first is the one in use).
    public struct Connection: Decodable, Sendable, Equatable {
        /// The engine's id for one its box keeps.
        public let id: String?
        public let state: State
        /// Who or what it is: "you@example.com", "API key", "From OPENAI_API_KEY".
        public let line: String

        private enum CodingKeys: String, CodingKey { case id, state, line }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            id = try container.decodeIfPresent(String.self, forKey: .id)
            state = State(rawValue: try container.decode(String.self, forKey: .state)) ?? .signedOut
            line = try container.decode(String.self, forKey: .line)
        }
    }

    public let state: State
    /// The account's status in a line, under its name.
    public let line: String
    public let actions: [Action]
    public let connections: [Connection]
    /// One you added beside a coding agent's own sign-in: listed with its agent whether or not
    /// it's signed in.
    public let added: Bool

    private enum CodingKeys: String, CodingKey { case state, line, actions, connections, added }

    /// One of a list's items, or nothing where this client can't read it: the rest still come.
    private struct Readable<Item: Decodable>: Decodable {
        let item: Item?
        init(from decoder: Decoder) throws { item = try? Item(from: decoder) }
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        state = State(rawValue: try container.decode(String.self, forKey: .state)) ?? .signedOut
        line = try container.decode(String.self, forKey: .line)
        actions = (try container.decodeIfPresent([Readable<Action>].self, forKey: .actions) ?? []).compactMap(\.item)
        connections = try container.decodeIfPresent([Connection].self, forKey: .connections) ?? []
        added = try container.decodeIfPresent(Bool.self, forKey: .added) ?? false
    }

    /// The action with `id`, where the box offers it now.
    public func action(_ id: String) -> Action? { actions.first { $0.id == id } }

    /// Its sign-in, where the box offers one now: Sign In, or Sign In Again.
    public var signIn: Action? { actions.first { $0.id == "signIn" || $0.id == "signInAgain" } }

    /// What the box says of one of its sign-ins: the one with the engine's `id`, where it has one,
    /// else the one at its place in the engine's list.
    public func connection(id: String?, at index: Int) -> Connection? {
        if let id, let found = connections.first(where: { $0.id == id }) { return found }
        return connections.indices.contains(index) && connections[index].id == nil ? connections[index] : nil
    }
}

/// Every account a computer's engine (or its coding agents' bridge) lists, with its ways in, as its
/// box says them (`kuvo/sign-in/accounts`): what Accounts' Sign In buttons and each card's hint go
/// by, which of them Accounts lists as providers (`listed`), each one's mark (`service`), and how
/// each stands with what can be done with it (`said`).
public struct SignInAccounts: Decodable, Sendable, Equatable {
    /// By account.
    public let accounts: [String: SignInMethods]
    /// The accounts Accounts lists as providers, as the box says (`sign-in.accounts.kinds`): not a
    /// web search service's, an MCP server's, nor the engine's own way to a plan a coding agent
    /// brings unless it's signed in. The box keeps the rule; a client lists these and no others.
    public let listed: Set<String>
    /// Each account's mark, by the plan limits' service ids ("copilot", "codex"); none for one
    /// drawn by its name's first letter.
    public let services: [String: String]
    /// How each account stands, its line and what can be done with it, as the box says
    /// (`sign-in.accounts.state`); none for one it hasn't said.
    public let said: [String: AccountSaid]

    private struct Entry: Decodable {
        let id: String
        let ways: SignInMethods
        let listed: Bool
        let service: String?
        let said: AccountSaid?
        private enum CodingKeys: String, CodingKey { case id, listed, service }
        init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            id = try container.decode(String.self, forKey: .id)
            listed = try container.decodeIfPresent(Bool.self, forKey: .listed) ?? false
            service = try container.decodeIfPresent(String.self, forKey: .service)
            ways = try SignInMethods(from: decoder)
            said = try? AccountSaid(from: decoder)
        }
    }

    private enum CodingKeys: String, CodingKey { case accounts }

    /// None said yet.
    public static let none = SignInAccounts(accounts: [:])

    public init(accounts: [String: SignInMethods], listed: Set<String> = [], services: [String: String] = [:], said: [String: AccountSaid] = [:]) {
        self.accounts = accounts
        self.listed = listed
        self.services = services
        self.said = said
    }

    public init(from decoder: Decoder) throws {
        let entries = try decoder.container(keyedBy: CodingKeys.self).decode([Entry].self, forKey: .accounts)
        accounts = Dictionary(entries.map { ($0.id, $0.ways) }) { first, _ in first }
        listed = Set(entries.filter(\.listed).map(\.id))
        services = Dictionary(entries.compactMap { entry in entry.service.map { (entry.id, $0) } }) { first, _ in first }
        said = Dictionary(entries.compactMap { entry in entry.said.map { (entry.id, $0) } }) { first, _ in first }
    }

    /// Whether Accounts lists `id` as a provider, as its box says; false where it hasn't said.
    public func lists(_ id: String) -> Bool { listed.contains(id) }

    /// Whether `id` has a way in a client can offer; false where its box hasn't said. (What
    /// Accounts offers for it is its `said` actions: this is for a sign-in offered elsewhere.)
    public func offersSignIn(_ id: String) -> Bool {
        accounts[id].map { !$0.methods.isEmpty } ?? false
    }

    /// How `id` stands, as its box says: not signed in where it hasn't said.
    public func state(_ id: String) -> AccountSaid.State { said[id]?.state ?? .signedOut }

    public static let route = "kuvo/sign-in/accounts"

    public static func path(agents: Bool) -> String {
        agents ? route + "?agents=1" : route
    }
}

/// How a started sign-in is going, as its box says (`kuvo/sign-in/status`): the engine's (or the
/// agents' bridge's) status, with what to say once it ended without signing in (`failure`).
public struct SignInStatus: Decodable, Sendable, Equatable {
    public let status: OAuthStatus.Status
    /// As the engine says it: how it's going while pending; why it failed, in its own words.
    public let message: String?
    /// The page, its words and its device code, when they come after it started.
    public let url: String?
    public let instructions: String?
    public let userCode: String?
    /// What a client shows once it `failed` or `expired`.
    public let failure: String?

    public static let route = "kuvo/sign-in/status"

    /// `wait`: seconds its box may wait for a change before it answers a sign-in still going, for
    /// a box asked through a process run each time (a machine's, over SSH).
    public static func path(integration: String, attempt: String, agents: Bool, wait: Int? = nil) -> String {
        route + "?" + SignInQuery.encoded([("integration", integration), ("attempt", attempt)] + (agents ? [("agents", "1")] : []) + (wait.map { [("wait", String($0))] } ?? []))
    }
}

enum SignInQuery {
    /// `name=value&…`, each value escaped whole ("+" and "&" too, which a query would read otherwise).
    static func encoded(_ items: [(String, String)]) -> String {
        var allowed = CharacterSet.urlQueryAllowed
        allowed.remove(charactersIn: "+&=?/#")
        return items.map { "\($0.0)=\($0.1.addingPercentEncoding(withAllowedCharacters: allowed) ?? $0.1)" }.joined(separator: "&")
    }
}
