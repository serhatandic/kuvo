import Foundation
import Testing
@testable import KuvoHelperKit

/// What belongs to the account whatever `$HOME` says: its crontab, the box's login agent in
/// launchd, and its keychains' search list and default (a test's keychain is never left in
/// either). Read before and after a suite, never changed.
struct AccountState: Equatable {
    var crontab: String
    var agentLoaded: Bool
    var agentFile: Bool
    var keychains: String

    static func now() -> AccountState {
        func run(_ program: String, _ arguments: [String]) -> (output: String, status: Int32) {
            guard FileManager.default.isExecutableFile(atPath: program) else { return ("", -1) }
            let process = Process()
            process.executableURL = URL(filePath: program)
            process.arguments = arguments
            let output = Pipe()
            process.standardOutput = output
            process.standardError = FileHandle.nullDevice
            process.standardInput = FileHandle.nullDevice
            let done = DispatchSemaphore(value: 0)
            process.terminationHandler = { _ in done.signal() }
            guard (try? process.run()) != nil else { return ("", -1) }
            let data = output.fileHandleForReading.readDataToEndOfFile()
            done.wait()
            return (String(decoding: data, as: UTF8.self), process.terminationStatus)
        }
        return AccountState(
            crontab: run("/usr/bin/crontab", ["-l"]).output,
            agentLoaded: run("/bin/launchctl", ["print", "gui/\(getuid())/\(BoxHelperBundle.launchAgent)"]).status == 0,
            agentFile: FileManager.default.fileExists(atPath: NSHomeDirectory() + "/Library/LaunchAgents/\(BoxHelperBundle.launchAgent).plist"),
            // (Which keychains are searched, and which is the default: their names, nothing in them.)
            keychains: run("/usr/bin/security", ["list-keychains", "-d", "user"]).output + run("/usr/bin/security", ["default-keychain", "-d", "user"]).output
        )
    }
}

/// Fails a suite whose run changed the account's real crontab, login agent or keychain list.
struct AccountUntouched: SuiteTrait, TestScoping {
    func provideScope(for test: Test, testCase: Test.Case?, performing function: @Sendable () async throws -> Void) async throws {
        let before = AccountState.now()
        try await function()
        let after = AccountState.now()
        if after != before { Issue.record("This run changed the account's real crontab, login agent or keychain list: \(before) became \(after)") }
    }
}
