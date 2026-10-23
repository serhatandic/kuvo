import Foundation
import Testing
@testable import OpenCodeKit

/// A box a newer Kuvo Code set up (one that updated itself, or another Mac's newer app) is left as
/// it is: what this app would put there isn't written over it. Run here, in a home folder of the
/// test's own, with the script a machine runs.
@Suite(.serialized) struct BoxVersionTests {
    let home = FileManager.default.temporaryDirectory.appending(path: "opencodekit-box-\(UUID().uuidString)")
    var bin: URL { home.appending(path: RemoteEngineInstaller.root).appending(path: "bin") }

    /// Runs a script that writes this app's file, as an install does: true when it wrote it.
    private func installs(over box: String?, mine: String) throws -> Bool {
        try FileManager.default.createDirectory(at: bin, withIntermediateDirectories: true)
        let marker = bin.appending(path: "box-version")
        try? FileManager.default.removeItem(at: marker)
        if let box { try Data(box.utf8).write(to: marker) }
        let file = bin.appending(path: "kuvo-link.js")
        try? FileManager.default.removeItem(at: file)
        let before = BoxVersion.mine
        BoxVersion.mine = mine
        defer { BoxVersion.mine = before }
        let process = Process()
        process.executableURL = URL(filePath: "/bin/sh")
        process.arguments = ["-c", BoxVersion.guardScript + "\n" + #"cat >"$HOME/\#(RemoteEngineInstaller.root)/bin/kuvo-link.js""#]
        process.environment = ["HOME": home.path, "PATH": "/usr/bin:/bin"]
        let input = Pipe()
        process.standardInput = input
        try process.run()
        input.fileHandleForWriting.write(Data("// this app's\n".utf8))
        try input.fileHandleForWriting.close()
        process.waitUntilExit()
        #expect(process.terminationStatus == 0)
        return FileManager.default.fileExists(atPath: file.path)
    }

    @Test func aNewerBoxIsLeftAsItIs() throws {
        defer { try? FileManager.default.removeItem(at: home) }
        #expect(try installs(over: nil, mine: "0.8.0"), "a box no Kuvo Code marked yet")
        #expect(try installs(over: "0.7.2", mine: "0.8.0"))
        #expect(try installs(over: "0.8.0", mine: "0.8.0"))
        #expect(try !installs(over: "0.8.1", mine: "0.8.0"))
        #expect(try !installs(over: "0.10.0", mine: "0.9.3"), "by number, not as text")
        #expect(try installs(over: "0.9.3", mine: "0.10.0"))
        #expect(try installs(over: "garbage", mine: "0.8.0"), "a marker that isn't a version marks nothing")
    }

    @Test func versionsCompareByNumber() {
        let before = BoxVersion.mine
        defer { BoxVersion.mine = before }
        BoxVersion.mine = "0.9.3"
        #expect(BoxVersion.isNewer("0.10.0"))
        #expect(!BoxVersion.isNewer("0.9.3"))
        #expect(!BoxVersion.isNewer("0.9.2"))
        #expect(!BoxVersion.isNewer(nil))
        BoxVersion.mine = "0.8.0; rm -rf ~"
        #expect(BoxVersion.mine == "0", "only digits and dots go into a script")
    }
}
