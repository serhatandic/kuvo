/// What a client offers when it has no setup to open.
public enum ComputerSetupOffer: String, Codable, Equatable, Sendable {
    case addComputer = "add-computer"
    case reinstallLocal = "reinstall-local"

    /// Count real, non-removed saved setup members, including those that are offline.
    public static func firstComputer(hasLocalBox: Bool, hasKnownComputers: Bool) -> Self? {
        hasLocalBox || hasKnownComputers ? nil : .addComputer
    }
}

/// A setup action carried by a successful removal or by the local installer's status.
public struct ComputerSetupOfferReply: Decodable, Sendable {
    public var offer: ComputerSetupOffer?
}
