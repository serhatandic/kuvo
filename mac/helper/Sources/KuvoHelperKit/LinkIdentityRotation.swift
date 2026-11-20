#if os(macOS)
import CryptoKit
import Foundation

/// The helper stores the three membership credentials in the existing static-key item's value.
/// Replacing that value publishes the new identity in one Keychain write.
struct LinkIdentityRotation: Codable {
    static let label = "Kuvo Code renewed phone link"
    var version = 1
    let attempt: String
    let previous: String
    let key: Data
    let secret: Data
    let sign: Data

    static func make(attempt: String, previous: String) -> Self {
        Self(attempt: attempt, previous: previous, key: Curve25519.KeyAgreement.PrivateKey().rawRepresentation,
             secret: SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) },
             sign: Curve25519.Signing.PrivateKey().rawRepresentation)
    }

    static func read(_ data: Data) -> Self? {
        guard let identity = try? JSONDecoder().decode(Self.self, from: data), identity.version == 1,
              validText(identity.attempt), validText(identity.previous),
              let key = publicKey(of: identity.key), key != identity.previous,
              identity.secret.count == 32, (try? Curve25519.Signing.PrivateKey(rawRepresentation: identity.sign)) != nil else { return nil }
        return identity
    }

    static func validText(_ value: String) -> Bool {
        guard value.utf8.count == 43,
              let bytes = Data(base64Encoded: value.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/") + "=") else { return false }
        return bytes.count == 32 && bytes.base64URLText == value
    }

    static func publicKey(of privateKey: Data) -> String? {
        (try? Curve25519.KeyAgreement.PrivateKey(rawRepresentation: privateKey))?.publicKey.rawRepresentation.base64URLText
    }

    /// The atomic label distinguishes even a corrupt rotated value of exactly 32 bytes
    /// from an old, valid raw key.
    static func isLegacy(_ data: Data, in keychain: HostKeychain, service: String) -> Bool {
        guard data.count == 32, let matching = keychain.items(service)?.first(where: { $0.account == "key" }) else { return false }
        return matching.label != label
    }

    var parts: [String: Data] { ["key": key, "secret": secret, "sign": sign] }
    var answer: HTTPResponse {
        var given: [String: Any] = parts.mapValues(\.base64URLText)
        given["rotation"] = true
        return .json(given)
    }
}
#endif
