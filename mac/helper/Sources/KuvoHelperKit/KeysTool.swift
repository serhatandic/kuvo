#if os(macOS)
import Darwin
import Foundation
import Security

/// `kuvo-keys`: the one-time hand-over of Kuvo Code's own Keychain items to this Mac's box, as the
/// box moves into a home of its own (protocol/PROTOCOL.md, "This Mac's box: its helper"). Kuvo Code
/// carries it, signed with Kuvo Code's own identifier and team, which is what the items it made
/// before go by: it reads them without macOS asking, as the app that made them would. The box's
/// move runs it; no app's window does Keychain work. It reaches only this account's `~/.kuvo-code`.
/// Signed differently from the Kuvo Code that made them (an Apple Development build after a
/// Developer ID one: the items' list names that one's certificate, not only its team), it isn't
/// one the items name, so reading each, macOS asks you whether `kuvo-keys` may (Always Allow): those
/// reads alone may ask, nothing else it does, and never to unlock a locked Keychain.
///
/// It reads what macOS keeps from every other program, so it does nothing for anything but the
/// box's move: only while a move holds its lock (`updates/rehome/lock/pid`), and only run by the
/// very process that holds it. Its own build is signed by a team, and gives the keys only to a
/// helper signed by that team, in the box's `bin` or the release its move unpacked
/// (`updates/install`).
///
/// **What that holds against, and what it doesn't (left for a decision, #713 §8e; until then no
/// `kuvo-keys` is signed with a real identity, scripts/build-helper.sh):** all of it rests on the
/// box's home being closed to whoever asks, which is true only of a command under the sandbox,
/// and of another user. A program of yours outside the sandbox can write the move's lock itself
/// and run this, and can put another program in the helper's place between `copy`'s check and
/// the copies being made (the check is of a path, and so is what the Keychain is told to trust).
///
/// - `kuvo-keys copy --home <box> --helper <Kuvo Helper.app>`: each of the app's items copied
///   under the helper's names, trusting only that helper (`KeychainMove.copy`).
/// - `kuvo-keys delete-originals --home <box>`: the app's own taken away, once the move can't go
///   back and each copy is there (`KeychainMove.deleteOriginals`).
/// - `kuvo-keys delete-originals --home <box> --password <site> --account <account>`:
///   removes only that saved sign-in after its vault password is read back; the expected password
///   is sent on stdin, and a changed original stays. Missing already is a successful retry.
/// - `kuvo-keys passwords --home <box>`: the sign-ins Kuvo Code 0.7 kept before the vault, said
///   down a pipe to the move, which gives them to the box's vault. Never written to a file.
///
/// One line of JSON on stdout. 0: done. 1: not done, with `error` (and `reason`). 2: asked wrongly.
///
/// COMPAT(0.7 kuvo-keys): hands the Keychain items and saved sign-ins Kuvo Code 0.5 to 0.7.2 and 0.8.0-beta.1 made to this Mac's box as it moves into its own home. Goes on the user's word, once no 0.7 Mac is expected to update.
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
        let passwordHost = option("--password")
        let passwordAccount = option("--account")
        var keychain = HostKeychain()
        #if DEBUG
        // Tests alone: a keychain file of their own, and the app they stand for (a debug build is
        // never signed as Kuvo Code).
        let pinned = false
        // (No one is there to answer macOS in a test.)
        let asks = false
        let named = option("--test-app")
        if let file = option("--test-keychain"), let password = option("--test-keychain-password") { keychain = HostKeychain(testFile: file, password: password) }
        guard keychain.isTests else { usage("A test build of kuvo-keys reads only a test's keychain: --test-keychain <file> --test-keychain-password <password>.") }
        #else
        let pinned = true
        let asks = true
        // Who it is, is what it's signed as: nothing it's told. And signed by a team: one signed
        // ad hoc (a local build) could be anyone's, and is never Kuvo Code to your Keychain.
        guard case .team? = BoxHelperBundle.Signer.ofThisProgram else { usage("kuvo-keys runs signed by Kuvo Code’s team.") }
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
        guard runByTheMove(of: home) else {
            usage("kuvo-keys works only for this Mac’s box’s move, run by it while it holds its lock.")
        }
        let move = KeychainMove(keychain: keychain, home: home, app: app, helper: helper)
        do {
            switch words {
            case ["copy"]:
                // Only Kuvo Helper itself is given the keys: signed by the team that signed this
                // (never one signed ad hoc, which anything could be, but in the tests' own build).
                // And only one in the box's `bin` or the release its move unpacked, where no command
                // under the sandbox can put another in its place between this check and the copies
                // being made. (A program of yours outside the sandbox can: left for a decision, #713 §8e.)
                guard let helperBundle, helperBundle.hasPrefix("/"), let trusted = inside(home, helperBundle, under: ["bin", "updates/install"]),
                      BoxHelperBundle.isGenuine(trusted, identifier: helper, adHocMatch: !pinned), let seal = BoxHelperBundle.cdhash(trusted) else {
                    usage("Copying needs \(BoxHelperBundle.name) itself, in this Mac’s box and signed by Kuvo Code’s team: --helper <its bundle>.")
                }
                let copied = try move.copy(trusting: trusted, askingIfNeeded: asks)
                // The same helper still, now that each copy names it: one changed meanwhile is said
                // (one changed and put back in between isn't seen).
                // (The journal stays at `copied`: the helper's `uncopy` takes away the copies it can,
                // and any it can't, made for something else, stay as leftovers that stop the next copy.)
                guard BoxHelperBundle.cdhash(trusted) == seal, BoxHelperBundle.isGenuine(trusted, identifier: helper, adHocMatch: !pinned) else {
                    say(["error": "\(BoxHelperBundle.name) changed while this Mac’s keys were copied for it: the copies aren’t to be used.", "reason": "helper-changed"], 1)
                }
                say(["copied": copied], 0)
            case ["delete-originals"]:
                if let passwordHost, let passwordAccount {
                    let data = FileHandle.standardInput.readDataToEndOfFile()
                    guard let given = try JSONSerialization.jsonObject(with: data) as? [String: String], let password = given["password"] else {
                        usage("Removing a saved sign-in needs its verified password on stdin.")
                    }
                    switch keychain.internetPassword(server: passwordHost, account: passwordAccount, creator: creator) {
                    case .missing: break // A previous try already removed it.
                    case .refused(let status): throw keychain.isLocked ? KeychainMove.Failure.locked("saved sign-in") : KeychainMove.Failure.refused("saved sign-in", status)
                    case .found(let original):
                        guard original == Data(password.utf8) else { throw KeychainMove.Failure.copyMissing("saved sign-in changed") }
                        let status = keychain.deleteInternetPassword(server: passwordHost, account: passwordAccount, creator: creator, verified: Data(password.utf8))
                        guard status == errSecSuccess || status == errSecItemNotFound else { throw KeychainMove.Failure.unchanged("saved sign-in", status) }
                    }
                } else {
                    guard passwordHost == nil && passwordAccount == nil else { usage("Name both the saved sign-in's site and account.") }
                    try move.deleteOriginals()
                }
                say([:], 0)
            case ["passwords"]:
                guard saysDownAPipe else { usage("kuvo-keys says your saved sign-ins only down a pipe to the move that runs it.") }
                say(try savedSignIns(keychain: keychain, askingIfNeeded: asks), 0)
            default:
                usage("Usage: kuvo-keys copy --home <box> --helper <\(BoxHelperBundle.bundleName)> | delete-originals --home <box> | passwords --home <box>")
            }
        } catch let failure as KeychainMove.Failure {
            // (`error` and `status` are for the move's log; what you're shown it words from `reason`.)
            var answer: [String: Any] = ["error": failure.localizedDescription, "reason": failure.reason]
            if case .refused(_, let status) = failure { answer["status"] = Int(status) }
            say(answer, 1)
        } catch {
            say(["error": error.localizedDescription, "reason": "failed"], 1)
        }
    }

    /// Whether the box's move runs this: its lock (`updates/rehome/lock/pid`, the box's own record
    /// of who holds it) names the process that started this one. Anything else that runs it, at
    /// any other time, gets nothing.
    static func runByTheMove(of home: URL) -> Bool {
        let record = home.appending(path: "updates/rehome/lock/pid")
        guard let text = try? String(contentsOf: record, encoding: .utf8),
              let holder = pid_t(text.trimmingCharacters(in: .whitespacesAndNewlines)), holder > 1, holder == getppid() else { return false }
        // And it's the process that took the lock, not one that got the number of a move that was
        // killed and left its lock behind: it started before the lock was written.
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        guard proc_pidinfo(holder, PROC_PIDTBSDINFO, 0, &info, size) == size,
              let written = (try? FileManager.default.attributesOfItem(atPath: record.path(percentEncoded: false)))?[.modificationDate] as? Date else { return false }
        let started = Date(timeIntervalSince1970: TimeInterval(info.pbi_start_tvsec) + TimeInterval(info.pbi_start_tvusec) / 1_000_000)
        return started <= written
    }

    /// `path` where it really is (links followed), if that's inside one of `folders` of `home` and
    /// every folder from `home` down to it, and every folder in it, is yours alone to change (yours,
    /// and closed to others' writes); else nil.
    static func inside(_ home: URL, _ path: String, under folders: [String]) -> URL? {
        func real(_ path: String) -> String? {
            guard let found = realpath(path, nil) else { return nil }
            defer { free(found) }
            return String(cString: found)
        }
        guard let root = real(home.path(percentEncoded: false)), let place = real(path),
              folders.contains(where: { place.hasPrefix(root + "/" + $0 + "/") }) else { return nil }
        func yoursAlone(_ path: String) -> Bool {
            guard let attributes = try? FileManager.default.attributesOfItem(atPath: path) else { return false }
            return (attributes[.ownerAccountID] as? NSNumber)?.uint32Value == getuid() && ((attributes[.posixPermissions] as? NSNumber)?.intValue ?? 0o777) & 0o022 == 0
        }
        var at = place
        while at != root {
            guard yoursAlone(at) else { return nil }
            at = (at as NSString).deletingLastPathComponent
            if at.count <= 1 { return nil }
        }
        // (Its own folders too: its signature seals what's in them, not who may change them.)
        guard let within = FileManager.default.enumerator(at: URL(filePath: place), includingPropertiesForKeys: [.isDirectoryKey]) else { return nil }
        for case let item as URL in within where (try? item.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true {
            guard yoursAlone(item.path(percentEncoded: false)) else { return nil }
        }
        return URL(filePath: place)
    }

    /// Whether standard output is a pipe to whatever started this (a pipe, or the pair of sockets
    /// some runtimes give a child for one): never a terminal, nor a file.
    static var saysDownAPipe: Bool {
        var status = stat()
        guard fstat(STDOUT_FILENO, &status) == 0 else { return false }
        let kind = status.st_mode & S_IFMT
        return kind == S_IFIFO || kind == S_IFSOCK
    }

    /// The sign-ins 0.7 kept as internet passwords, for the box's move to take into its vault:
    /// `{"v": 1, "logins": [{host, account, password, modified?}]}`. One the Keychain lists but
    /// won't give out stops it with nothing said: none is left behind unsaid. The Keychain's own
    /// stay as they are.
    ///
    /// They're said only down a pipe to the move that runs this (which holds them in memory and
    /// gives each to the box's own server): never written to a file, where any program of yours
    /// outside the sandbox could read what the Keychain kept from all but Kuvo Code.
    static func savedSignIns(keychain: HostKeychain, askingIfNeeded: Bool = false) throws -> [String: Any] {
        guard !keychain.isLocked else { throw KeychainMove.Failure.locked("your saved sign-ins") }
        guard let found = keychain.internetPasswords(creator: creator) else {
            throw KeychainMove.Failure.refused("your saved sign-ins", errSecInteractionNotAllowed)
        }
        var logins: [[String: Any]] = []
        for login in found {
            switch keychain.internetPassword(server: login.server, account: login.account, creator: creator, askingIfNeeded: askingIfNeeded) {
            case .found(let data):
                var entry: [String: Any] = ["host": login.server, "account": login.account, "password": String(decoding: data, as: UTF8.self)]
                if let modified = login.modified { entry["modified"] = Int(modified.timeIntervalSince1970 * 1000) }
                logins.append(entry)
            // (Gone meanwhile.)
            case .missing: continue
            case .refused(let status):
                throw keychain.isLocked ? KeychainMove.Failure.locked("your saved sign-ins") : KeychainMove.Failure.refused("\(login.account) at \(login.server)", status)
            }
        }
        return ["v": 1, "logins": logins]
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
    /// One word for what's reading the answer (the box's move), which decides what's next by it:
    /// `locked` only when the Keychain is; `not-allowed` when macOS kept an item from this program
    /// (it would have asked, or you didn't allow it); `refused`, anything else it said.
    var reason: String {
        switch self {
        case .locked: "locked"
        case .refused(_, let status): [errSecInteractionNotAllowed, errSecAuthFailed, errSecUserCanceled].contains(status) ? "not-allowed" : "refused"
        case .unchanged: "unchanged"
        case .copyMissing: "copy-missing"
        case .originalsRemain: "originals-remain"
        case .journal: "journal"
        case .leftovers: "leftovers"
        }
    }
}
#endif
