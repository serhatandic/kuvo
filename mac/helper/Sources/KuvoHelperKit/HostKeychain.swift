#if os(macOS)
import Foundation
import Security

/// Kuvo Code's items for a Mac's box in the legacy (file) keychain, where they've always been: a
/// generic password each, by service (`<bundle ID>.phone-link`, `.integrations`, `.vault`) and
/// account. Used by the box's helper (its own items) and by the app's keeper (copying its items for
/// the helper, #333 piece 4). It never asks you anything: a read macOS would ask about (a locked
/// Keychain, a program not in the item's list) is refused, and said so. An item is made with the
/// programs that may read it named at creation (`SecAccessCreate`), never by changing the list of
/// one that's there, which macOS asks you about once per item. (The one exception: a read that may
/// ask whether this program may use an item, `askingIfNeeded`, for `kuvo-keys` reading what an
/// earlier Kuvo Code signed differently made. A locked Keychain is never asked to be unlocked.)
///
/// Tests (and only tests) give it a keychain file of their own, made with `security create-keychain`
/// in their folder: it then reads and writes only that file, never your login keychain.
public struct HostKeychain: Sendable {
    /// What a read found.
    public enum Read: Equatable, Sendable {
        case found(Data)
        case missing
        /// The Keychain wouldn't give it out without asking (`OSStatus`): locked, or this program
        /// isn't one the item names.
        case refused(OSStatus)

        public var data: Data? {
            if case .found(let data) = self { return data }
            return nil
        }
    }

    /// One item, as listed (no secret read).
    public struct Item: Equatable, Sendable {
        public var account: String
        public var label: String?
    }

    /// A test's keychain file and its password; nil: the login keychain, as macOS finds it.
    private let file: String?
    private let password: String?

    /// The Keychain as macOS finds it for you (your login keychain).
    public init() {
        file = nil
        password = nil
    }

    /// A test's own keychain file, unlocked with `password`, and nothing else.
    public init(testFile: String, password: String) {
        file = testFile
        self.password = password
    }

    /// Whether it reads only a test's file.
    public var isTests: Bool { file != nil }

    /// The test's file, opened and unlocked (each time: a `SecKeychain` isn't one to share).
    private func testKeychain() -> SecKeychain? {
        guard let file else { return nil }
        var keychain: SecKeychain?
        guard SecKeychainOpen(file, &keychain) == errSecSuccess, let keychain else { return nil }
        if let password { _ = SecKeychainUnlock(keychain, UInt32(password.utf8.count), password, true) }
        return keychain
    }

    /// `query`, kept to the test's file where there is one.
    private func scoped(_ query: [String: Any], adding: Bool = false) -> [String: Any]? {
        var query = query
        query[kSecUseDataProtectionKeychain as String] = false
        guard file != nil else { return query }
        guard let keychain = testKeychain() else { return nil }
        if adding { query[kSecUseKeychain as String] = keychain } else { query[kSecMatchSearchList as String] = [keychain] }
        return query
    }

    /// Whether the keychain the items are in is locked now: a read then would ask you to unlock it.
    public var isLocked: Bool {
        var keychain: SecKeychain?
        if file != nil { keychain = testKeychain() } else { _ = SecKeychainCopyDefault(&keychain) }
        var status: SecKeychainStatus = 0
        guard let keychain, SecKeychainGetStatus(keychain, &status) == errSecSuccess else { return false }
        return status & SecKeychainStatus(kSecUnlockStateStatus) == 0
    }

    private static func base(_ service: String, _ account: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: account]
    }

    /// An item's secret. `askingIfNeeded`: where the item doesn't name this program, macOS may ask
    /// you whether it may use it (`secret`'s rules).
    public func read(_ service: String, _ account: String, askingIfNeeded: Bool = false) -> Read {
        secret(Self.base(service, account), askingIfNeeded: askingIfNeeded)
    }

    /// The secret `query` finds. A locked Keychain is refused, never asked to be unlocked. Asking
    /// where needed: tried first without asking (nothing's asked of an item that names this
    /// program); refused, macOS may ask you, in a process that otherwise never asks (`neverAsk`),
    /// searching only the item's own keychain, and only while that's unlocked; and what you allowed
    /// must then hold without asking (Always Allow, which the steps after it need: a deletion, a
    /// later run), or it's refused as macOS would refuse it.
    private func secret(_ query: [String: Any], askingIfNeeded: Bool) -> Read {
        if isLocked { return .refused(errSecInteractionNotAllowed) }
        var query = query
        query[kSecReturnData as String] = true
        guard var query = scoped(query) else { return .refused(errSecNoSuchKeychain) }
        let first = Self.copySecret(query)
        guard askingIfNeeded, case .refused = first, let home = keychain(of: query), !Self.isLocked(home) else { return first }
        query[kSecMatchSearchList as String] = [home]
        guard case .found = Self.asking({ Self.copySecret(query) }) else { return first }
        return Self.copySecret(query)
    }

    private static func copySecret(_ query: [String: Any]) -> Read {
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        switch status {
        case errSecSuccess: return (result as? Data).map(Read.found) ?? .missing
        case errSecItemNotFound: return .missing
        default: return .refused(status)
        }
    }

    /// The keychain the item `query` finds is in (no secret read, so nothing's asked).
    private func keychain(of query: [String: Any]) -> SecKeychain? {
        var query = query
        query[kSecReturnData as String] = nil
        query[kSecReturnRef as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let result,
              CFGetTypeID(result) == SecKeychainItemGetTypeID() else { return nil }
        var keychain: SecKeychain?
        // swiftlint:disable:next force_cast
        guard SecKeychainItemCopyKeychain(result as! SecKeychainItem, &keychain) == errSecSuccess else { return nil }
        return keychain
    }

    private static func isLocked(_ keychain: SecKeychain) -> Bool {
        var status: SecKeychainStatus = 0
        guard SecKeychainGetStatus(keychain, &status) == errSecSuccess else { return true }
        return status & SecKeychainStatus(kSecUnlockStateStatus) == 0
    }

    /// What listing a service found: its items, or what the Keychain said instead.
    public enum Listing: Equatable, Sendable {
        case items([Item])
        case refused(OSStatus)
    }

    /// The items of `service`, by account (and label), with no secret read, so it never asks.
    public func list(_ service: String) -> Listing {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
            kSecReturnAttributes as String: true, kSecMatchLimit as String: kSecMatchLimitAll,
        ]
        guard let query = scoped(query) else { return .refused(errSecNoSuchKeychain) }
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        switch status {
        case errSecSuccess:
            let found = (result as? [[String: Any]]) ?? []
            return .items(found.compactMap { item in
                (item[kSecAttrAccount as String] as? String).map { Item(account: $0, label: item[kSecAttrLabel as String] as? String) }
            }.sorted { $0.account < $1.account })
        case errSecItemNotFound: return .items([])
        default: return .refused(status)
        }
    }

    /// The items of `service` (`list`): nil when the Keychain won't list them.
    public func items(_ service: String) -> [Item]? {
        if case .items(let items) = list(service) { return items }
        return nil
    }

    /// What the Keychain says of an item, no secret read: `errSecSuccess` (there),
    /// `errSecItemNotFound`, or why it won't say.
    public func look(_ service: String, _ account: String) -> OSStatus {
        var query = Self.base(service, account)
        query[kSecReturnAttributes as String] = true
        guard let query = scoped(query) else { return errSecNoSuchKeychain }
        var result: CFTypeRef?
        return SecItemCopyMatching(query as CFDictionary, &result)
    }

    /// Whether an item is there (no secret read); nil when the Keychain won't say.
    public func exists(_ service: String, _ account: String) -> Bool? {
        switch look(service, account) {
        case errSecSuccess: true
        case errSecItemNotFound: false
        default: nil
        }
    }

    /// A new item (`errSecDuplicateItem` where there's one). `trusting`: the programs (app bundles,
    /// by their designated requirement) that may read it without asking, named as it's made; nil,
    /// the program making it, as macOS does by itself.
    @discardableResult
    public func add(_ service: String, _ account: String, _ data: Data, label: String?, trusting programs: [URL]? = nil) -> OSStatus {
        var item = Self.base(service, account)
        item[kSecValueData as String] = data
        if let label { item[kSecAttrLabel as String] = label }
        // Bots work overnight, with the Mac locked: readable once it's been unlocked since it started.
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        if let programs {
            var trusted: [SecTrustedApplication] = []
            for program in programs {
                var application: SecTrustedApplication?
                let status = SecTrustedApplicationCreateFromPath(program.path(percentEncoded: false), &application)
                guard status == errSecSuccess, let application else { return status }
                trusted.append(application)
            }
            var access: SecAccess?
            let status = SecAccessCreate((label ?? service) as CFString, trusted as CFArray, &access)
            guard status == errSecSuccess, let access else { return status }
            item[kSecAttrAccess as String] = access
        }
        guard let item = scoped(item, adding: true) else { return errSecNoSuchKeychain }
        return SecItemAdd(item as CFDictionary, nil)
    }

    /// An item's secret changed, or the item made (by and for this program) where there's none.
    @discardableResult
    public func set(_ service: String, _ account: String, _ data: Data, label: String?) -> OSStatus {
        let updated = update(service, account, [kSecValueData as String: data])
        guard updated == errSecItemNotFound else { return updated }
        return add(service, account, data, label: label)
    }

    /// Replaces the data and label in one write. A missing item stays missing.
    func replace(_ service: String, _ account: String, _ data: Data, label: String) -> OSStatus {
        update(service, account, [kSecValueData as String: data, kSecAttrLabel as String: label])
    }

    private func update(_ service: String, _ account: String, _ attributes: [String: Any]) -> OSStatus {
        guard let query = scoped(Self.base(service, account)) else { return errSecNoSuchKeychain }
        return SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    }

    /// An item taken away (`errSecItemNotFound` where there's none). Only a program the item names
    /// may: macOS refuses another without asking.
    @discardableResult
    public func delete(_ service: String, _ account: String) -> OSStatus {
        guard let query = scoped(Self.base(service, account)) else { return errSecNoSuchKeychain }
        return SecItemDelete(query as CFDictionary)
    }

    // MARK: Sign-ins Kuvo Code 0.7 kept (internet passwords)

    /// One sign-in, as listed (no secret read).
    public struct InternetPassword: Equatable, Sendable {
        public var server: String
        public var account: String
        public var modified: Date?
    }

    /// The internet passwords marked with `creator`, with no secret read: nil when the Keychain
    /// won't list them (`internetPasswordsListed`: why).
    public func internetPasswords(creator: OSType) -> [InternetPassword]? {
        try? internetPasswordsListed(creator: creator).get()
    }

    /// What the Keychain won't list, as it said it.
    public struct Refusal: Error, Equatable {
        public var status: OSStatus
    }

    public func internetPasswordsListed(creator: OSType) -> Result<[InternetPassword], Refusal> {
        let query: [String: Any] = [
            kSecClass as String: kSecClassInternetPassword, kSecAttrCreator as String: creator,
            kSecReturnAttributes as String: true, kSecMatchLimit as String: kSecMatchLimitAll,
        ]
        guard let query = scoped(query) else { return .failure(Refusal(status: errSecNoSuchKeychain)) }
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        switch status {
        case errSecSuccess:
            return .success(((result as? [[String: Any]]) ?? []).compactMap { item in
                guard let server = item[kSecAttrServer as String] as? String else { return nil }
                return InternetPassword(server: server, account: item[kSecAttrAccount as String] as? String ?? "", modified: item[kSecAttrModificationDate as String] as? Date)
            }.sorted { ($0.server, $0.account) < ($1.server, $1.account) })
        case errSecItemNotFound: return .success([])
        default: return .failure(Refusal(status: status))
        }
    }

    /// One such sign-in's secret.
    /// (`askingIfNeeded`, as `read`'s.)
    public func internetPassword(server: String, account: String, creator: OSType, askingIfNeeded: Bool = false) -> Read {
        secret([
            kSecClass as String: kSecClassInternetPassword, kSecAttrCreator as String: creator, kSecAttrServer as String: server,
            kSecAttrAccount as String: account, kSecMatchLimit as String: kSecMatchLimitOne,
        ], askingIfNeeded: askingIfNeeded)
    }

    /// Remove only the earlier app's named sign-in, after its vault copy was verified.
    public func deleteInternetPassword(server: String, account: String, creator: OSType, verified: Data) -> OSStatus {
        if isLocked { return errSecInteractionNotAllowed }
        let query: [String: Any] = [
            kSecClass as String: kSecClassInternetPassword, kSecAttrCreator as String: creator,
            kSecAttrServer as String: server, kSecAttrAccount as String: account,
            kSecReturnData as String: true, kSecReturnPersistentRef as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        guard let query = scoped(query) else { return errSecNoSuchKeychain }
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess else { return status }
        guard let item = result as? [String: Any], let data = item[kSecValueData as String] as? Data,
              data == verified, let reference = item[kSecValuePersistentRef as String] as? Data else { return errSecVerifyFailed }
        // Delete exactly the item read, never other paths/protocols at the same site/account.
        guard let deletion = scoped([kSecValuePersistentRef as String: reference]) else { return errSecNoSuchKeychain }
        return SecItemDelete(deletion as CFDictionary)
    }

    /// For a process with no one to answer macOS (the helper, a test): anything that would ask you
    /// fails at once instead.
    public static func neverAsk() {
        SecKeychainSetUserInteractionAllowed(false)
    }

    /// `body`, with macOS allowed to ask you while it runs where `allowed`, and as it was after.
    /// (Process-wide: the programs that use it make one Keychain call at a time.)
    private static func asking<T>(_ body: () -> T) -> T {
        var was: DarwinBoolean = false
        _ = SecKeychainGetUserInteractionAllowed(&was)
        SecKeychainSetUserInteractionAllowed(true)
        defer { SecKeychainSetUserInteractionAllowed(was.boolValue) }
        return body()
    }
}
#endif
