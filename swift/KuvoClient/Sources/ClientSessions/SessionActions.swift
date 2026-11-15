import Foundation

/// What a session's menu offers now, as its box says it (`GET kuvo/sessions/<id>/actions`,
/// PROTOCOL.md "A session's actions"): Pin, Rename, Archive and Delete, each with its button's
/// words and, where a client asks first, the confirm in the box's words (Delete's says what goes
/// with its worktree). The box does each through its own route and refuses one moving to another
/// computer; a client draws the menu and what the box answers, and works out none of it.
public struct SessionActions: Decodable, Sendable, Equatable {
    /// The question a client asks first, what it comes to, and the words of the button that goes
    /// ahead.
    public struct Confirm: Decodable, Sendable, Equatable {
        public let title: String
        public let message: String
        public let label: String
    }

    /// One thing the menu offers.
    public struct Action: Decodable, Sendable, Equatable, Identifiable {
        /// `pin`, `unpin`, `rename`, `archive`, `unarchive`, `delete`; a client leaves out one it
        /// has no control for.
        public let id: String
        /// Its button's words (never with an ellipsis: a client adds its own where it confirms).
        public let label: String
        /// It destroys something: a client draws it so.
        public let destructive: Bool
        /// There where a client asks first.
        public let confirm: Confirm?

        private enum CodingKeys: String, CodingKey { case id, label, destructive, confirm }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            id = try container.decode(String.self, forKey: .id)
            label = try container.decode(String.self, forKey: .label)
            destructive = try container.decodeIfPresent(Bool.self, forKey: .destructive) ?? false
            confirm = try container.decodeIfPresent(Confirm.self, forKey: .confirm)
        }
    }

    public let actions: [Action]
    /// Why none of it can be done now (it's moving to another computer), in words to show.
    public let held: String?

    /// The one with `id`, where the menu offers it.
    public func action(_ id: String) -> Action? { actions.first { $0.id == id } }

    /// Where Delete All asks its box what deleting several at once comes to (`POST {sessions: [<id>]}`,
    /// answered with a `Confirm`), before each is deleted on its own route.
    public static let deletesRoute = "kuvo/sessions/deletes"

    /// Its route on the session's box: what the menu offers (`actions`), or one of them (`pin`,
    /// `archive`, `rename`; Delete is `DELETE` on the session's own, `action` nil).
    public static func route(_ session: String, _ action: String? = nil) -> String {
        let part = session.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/?#+"))) ?? session
        return "kuvo/sessions/\(part)" + (action.map { "/\($0)" } ?? "")
    }
}
