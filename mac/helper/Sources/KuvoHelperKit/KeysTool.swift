#if os(macOS)
import Darwin
import Foundation
import Security

/// `kuvo-keys`: the one-time hand-over of Kuvo Code's own Keychain items to this Mac's box, as the
/// box moves into a home of its own (protocol/PROTOCOL.md, "This Mac's box: its helper"). Kuvo Code
/// carries it, signed with Kuvo Code's own identifier and team, which is what the items it made
/// before go by: it reads them without macOS asking, as the app that made them would. The box's
/// move runs it; no app's window does Keychain work. It never asks you anything, and reaches only
/// this account's `~/.kuvo-code`.
///
/// - `kuvo-keys copy --home <box> --helper <Kuvo Helper.app>`: each of the app's items copied
///   under the helper's names, trusting only that helper (`KeychainMove.copy`).
/// - `kuvo-keys delete-originals --home <box>`: the app's own taken away, once the move can't go
///   back and each copy is there (`KeychainMove.deleteOriginals`).
/// - `kuvo-keys passwords --home <box>`: the sign-ins Kuvo Code 0.7 kept before the vault, written
///   to `updates/rehome/passwords.json` for the box to take into its vault, then delete.
///
/// One line of JSON on stdout. 0: done. 1: not done, with `error` (and `reason`). 2: asked wrongly.
public enum KeysTool {
    /// 'KUVO', the creator code on every sign-in Kuvo Code 0.7 saved.
    static let creator: OSType = 0x4B55_564F

    public static func main(_ arguments: [String]) -> Never {
        HostKeychain.neverAsk()
        var words = Array(arguments.dropFirst())
        func option(_ name: String) -> String? {
            guard let index = words.firstIndex(of: name), index + 1 < words.count else { return nil }
            defer { words.removeSubrange(index...(index + 1)) }
            return words[index + 1]
        }
        let given = option("--home")
        let helperBundle = option("--helper")
        var keychain = HostKeychain()
        #if DEBUG
        // Tests alone: a keychain file of their own, and the app they stand for (a debug build is
        // never signed as Kuvo Code).
        let pinned = false
        let named = option("--test-app")
        if let file = option("--test-keychain"), let password = option("--test-keychain-password") { keychain = HostKeychain(testFile: file, password: password) }
        guard keychain.isTests else { usage("A test build of kuvo-keys reads only a test's keychain: --test-keychain <file> --test-keychain-password <password>.") }
        #else
        let pinned = true
        // Who it is, is what it's signed as: nothing it's told.
        let named = BoxHelperBundle.identifier()
        #endif
        guard let app = named, !app.isEmpty, BoxHelperBundle.appID(ofHelper: app) == nil else {
            usage("kuvo-keys runs signed as Kuvo Code itself.")
        }
        let helper = BoxHelperBundle.helperID(ofApp: app)
        let home: URL
        switch BoxHelper.box(given: given, bundle: Bundle.main.bundleURL, accountHome: BoxHelper.accountHome(), pinned: pinned, keeping: false) {
        case .success(let found): home = found
        case .failure(let refused): usage(refused.why)
        }
        let move = KeychainMove(keychain: keychain, home: home, app: app, helper: helper)
        do {
            switch words {
            case ["copy"]:
                // Only Kuvo Helper itself is given the keys: signed by the team that signed this
                // (never one signed ad hoc, which anything could be, but in the tests' own build).
                guard let helperBundle, helperBundle.hasPrefix("/"),
                      BoxHelperBundle.isGenuine(URL(filePath: helperBundle), identifier: helper, adHocMatch: !pinned) else {
                    usage("Copying needs \(BoxHelperBundle.name) itself, signed by Kuvo Code’s team: --helper <its bundle>.")
                }
                say(["copied": try move.copy(trusting: URL(filePath: helperBundle))], 0)
            case ["delete-originals"]:
                try move.deleteOriginals()
                say([:], 0)
            case ["passwords"]:
                say(["passwords": try handOverPasswords(keychain: keychain, home: home)], 0)
            default:
                usage("Usage: kuvo-keys copy --home <box> --helper <\(BoxHelperBundle.bundleName)> | delete-originals --home <box> | passwords --home <box>")
            }
        } catch let failure as KeychainMove.Failure {
            say(["error": failure.localizedDescription, "reason": failure.reason], 1)
        } catch {
            say(["error": error.localizedDescription, "reason": "failed"], 1)
        }
    }

    /// Where the sign-ins go for the box (`0600`, in the folder the move keeps).
    static func passwordsFile(in home: URL) -> URL { home.appending(path: "updates/rehome/passwords.json") }

    /// The sign-ins 0.7 kept as internet passwords, written whole for the box; how many. One the
    /// Keychain lists but won't give out stops it with nothing written: none is left behind
    /// unsaid. The Keychain's own stay as they are.
    static func handOverPasswords(keychain: HostKeychain, home: URL) throws -> Int {
        guard !keychain.isLocked, let found = keychain.internetPasswords(creator: creator) else {
            throw KeychainMove.Failure.refused("your saved sign-ins", errSecInteractionNotAllowed)
        }
        var logins: [[String: Any]] = []
        for login in found {
            switch keychain.internetPassword(server: login.server, account: login.account, creator: creator) {
            case .found(let data):
                var entry: [String: Any] = ["host": login.server, "account": login.account, "password": String(decoding: data, as: UTF8.self)]
                if let modified = login.modified { entry["modified"] = Int(modified.timeIntervalSince1970 * 1000) }
                logins.append(entry)
            // (Gone meanwhile.)
            case .missing: continue
            case .refused(let status): throw KeychainMove.Failure.refused("\(login.account) at \(login.server)", status)
            }
        }
        let data = try JSONSerialization.data(withJSONObject: ["v": 1, "logins": logins], options: [.sortedKeys])
        try PrivateFile.write(data, to: passwordsFile(in: home))
        return logins.count
    }

    private static func say(_ answer: [String: Any], _ status: Int32) -> Never {
        let data = (try? JSONSerialization.data(withJSONObject: answer, options: [.sortedKeys])) ?? Data("{}".utf8)
        FileHandle.standardOutput.write(data + Data("\n".utf8))
        if let error = answer["error"] as? String { FileHandle.standardError.write(Data((error + "\n").utf8)) }
        exit(status)
    }

    private static func usage(_ message: String) -> Never {
        FileHandle.standardError.write(Data((message + "\n").utf8))
        exit(2)
    }
}

extension KeychainMove.Failure {
    /// One word for what's reading the answer (the box's move), which decides what's next by it.
    var reason: String {
        switch self {
        case .refused: "refused"
        case .unchanged: "unchanged"
        case .copyMissing: "copy-missing"
        case .originalsRemain: "originals-remain"
        case .journal: "journal"
        case .leftovers: "leftovers"
        }
    }
}
#endif
