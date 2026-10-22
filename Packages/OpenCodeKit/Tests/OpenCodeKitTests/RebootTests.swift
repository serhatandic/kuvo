import Foundation
import Testing
@testable import OpenCodeKit

/// A machine that restarted: `kuvo-start`, from its crontab, starts the engine again as the Mac last
/// started it (its settings, keys and environment, with new passwords), and the Mac finds it as its
/// own. One stopped on purpose stays stopped, one still running is left alone, and two at once make
/// one. Run here, in a home folder of the test's own, with a stand-in engine, and stand-ins for what
/// Linux has that a Mac doesn't (`setsid`, `/proc`). The live suite does the same on a real machine.
@Suite(.serialized) struct RebootTests {
    let home: URL
    let tools: URL
    var root: URL { home.appending(path: RemoteEngineInstaller.root) }
    var run: URL { root.appending(path: "run") }

    init() throws {
        home = FileManager.default.temporaryDirectory.appending(path: "opencodekit-reboot-\(UUID().uuidString)")
        tools = home.appending(path: "tools")
        try FileManager.default.createDirectory(at: tools, withIntermediateDirectories: true)
        // `setsid` starts what it's given (a Mac has none); `grep` reads `/proc/<pid>/cmdline` as
        // Linux has it, from `ps`.
        try tool("setsid", #"exec "$@""#)
        try tool("grep", """
            n=$#; i=0; file=
            for a in "$@"; do i=$((i + 1)); if [ $i -lt $n ]; then set -- "$@" "$a"; else file=$a; fi; done
            shift $n
            case "$file" in
              /proc/[0-9]*/cmdline) pid=${file#/proc/}; pid=${pid%/cmdline}; ps -p "$pid" -o args= 2>/dev/null | /usr/bin/grep "$@" ;;
              *) exec /usr/bin/grep "$@" "$file" ;;
            esac
            """)
        // The engine: says it's listening, notes how it was started, and runs until stopped.
        let engine = root.appending(path: "engine/\(OpenCodeEngine.version)/opencode")
        try FileManager.default.createDirectory(at: engine.deletingLastPathComponent(), withIntermediateDirectories: true)
        try write(engine, """
            #!/bin/sh
            env | sort >"$HOME/engine-$$.env"
            echo "server listening on http://127.0.0.1:$3"
            while :; do /bin/sleep 1; done
            """, executable: true)
        // What the Mac's last start left: how it started it, and its notes.
        try FileManager.default.createDirectory(at: run, withIntermediateDirectories: true)
        try write(run.appending(path: "launch.env"), "PATH=\(tools.path):/usr/bin:/bin\nLANG=tr_TR.UTF-8\nKUVO_TZ=Europe/Istanbul\n")
        try write(run.appending(path: "launch.notes"), #"{"mac":"test-mac","settings":"abc"}"#)
        let bin = root.appending(path: "bin")
        try FileManager.default.createDirectory(at: bin, withIntermediateDirectories: true)
        try write(bin.appending(path: "kuvo-start"), RemoteServer.bootScript, executable: true)
        try write(bin.appending(path: "kuvo-launch"), RemoteServer.bootLaunchScript, executable: true)
        // Its keys, which a restart keeps.
        let keys = root.appending(path: "keys")
        try FileManager.default.createDirectory(at: keys, withIntermediateDirectories: true)
        try write(keys.appending(path: "KUVO_KEY_TEST"), "secret-key")
        try write(keys.appending(path: "settings.json"), #"{"mcp":{}}"#)
    }

    private func tool(_ name: String, _ body: String) throws {
        try write(tools.appending(path: name), "#!/bin/sh\n" + body + "\n", executable: true)
    }

    private func write(_ file: URL, _ text: String, executable: Bool = false) throws {
        try Data(text.utf8).write(to: file)
        if executable { try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: file.path) }
    }

    /// Everything it started (the engine, its watchdog) stopped, and its folder gone.
    private func cleanUp() {
        let stop = Process()
        stop.executableURL = URL(filePath: "/usr/bin/pkill")
        stop.arguments = ["-f", home.path]
        try? stop.run()
        stop.waitUntilExit()
        try? FileManager.default.removeItem(at: home)
    }

    /// The machine's engine record as a restart left it, as it was before (a dead number).
    private func ranBefore(pid: Int32 = 999_999) throws {
        try write(run.appending(path: "engine.json"), #"{"pid":\#(pid),"port":1,"password":"old","version":"\#(OpenCodeEngine.version)","notes":{"mac":"test-mac","settings":"abc"}}"#)
    }

    /// `kuvo-start` as cron runs it: next to nothing in its environment.
    @discardableResult
    private func start() throws -> Int32 {
        let process = Process()
        process.executableURL = URL(filePath: "/bin/sh")
        process.arguments = [root.appending(path: "bin/kuvo-start").path]
        process.environment = ["HOME": home.path, "PATH": "\(tools.path):/usr/bin:/bin"]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        return process.terminationStatus
    }

    private func record() throws -> RemoteServer.Engine? {
        guard let data = try? Data(contentsOf: run.appending(path: "engine.json")) else { return nil }
        return try JSONDecoder().decode(RemoteServer.Engine.self, from: data)
    }

    /// The environments engines were started with, one for each.
    private func started() throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: home.path).filter { $0.hasPrefix("engine-") }
            .map { try String(contentsOf: home.appending(path: $0), encoding: .utf8) }
    }

    private func alive(_ pid: Int) -> Bool { kill(pid_t(pid), 0) == 0 }

    @Test func itComesBackAsTheMacLastStartedIt() throws {
        defer { cleanUp() }
        try ranBefore()
        #expect(try start() == 0)
        let engine = try #require(try record(), "it wrote where it runs")
        #expect(alive(engine.pid))
        #expect(engine.notes == ["mac": "test-mac", "settings": "abc"], "the Mac finds it as its own")
        #expect(engine.password != "old" && engine.password.count == 32, "with a new password")
        #expect((20000..<60000).contains(engine.port))
        let environment = try #require(try started().first)
        // As the Mac started it, under cron's bare environment: its language and time zone, its
        // private settings and its password, and nothing of the start itself.
        #expect(environment.contains("LANG=tr_TR.UTF-8") && environment.contains("KUVO_TZ=Europe/Istanbul"))
        #expect(environment.contains("OPENCODE_CONFIG=\(root.path)/keys/settings.json"))
        #expect(environment.contains("OPENCODE_PASSWORD=\(engine.password)"))
        #expect(!environment.contains("KUVO_BOOT") && !environment.contains("KUVO_RECORD") && !environment.contains("BUN_BE_BUN"))
        // Its keys kept.
        #expect(try String(contentsOf: root.appending(path: "keys/KUVO_KEY_TEST"), encoding: .utf8) == "secret-key")
    }

    @Test func oneStoppedOnPurposeStaysStopped() throws {
        defer { cleanUp() }
        // No record: it was stopped (Remove, or a day without the app), not cut off.
        #expect(try start() == 0)
        #expect(try record() == nil)
        #expect(try started().isEmpty)
    }

    @Test func oneTheMacNeverStartedStaysStopped() throws {
        defer { cleanUp() }
        try ranBefore()
        try FileManager.default.removeItem(at: run.appending(path: "launch.env"))
        #expect(try start() == 0)
        #expect(try started().isEmpty)
    }

    @Test func oneStillRunningIsLeftAlone() throws {
        defer { cleanUp() }
        try ranBefore()
        try start()
        let running = try #require(try record())
        try start()
        #expect(try record() == running)
        #expect(try started().count == 1)
    }

    @Test func aNumberAnotherProcessTookAfterTheRestartIsntTheEngine() throws {
        defer { cleanUp() }
        let other = Process()
        other.executableURL = URL(filePath: "/bin/sleep")
        other.arguments = ["30"]
        try other.run()
        defer { other.terminate() }
        try ranBefore(pid: other.processIdentifier)
        #expect(try start() == 0)
        let engine = try #require(try record())
        #expect(engine.pid != Int(other.processIdentifier))
        #expect(try started().count == 1)
    }

    /// The watchdog's rounds made quick (a minute each on a machine), each counted by the
    /// wake-up sender it runs.
    private func quickRounds() throws {
        try tool("sleep", "exec /bin/sleep 0.02")
        try write(root.appending(path: "bin/kuvo-wake"), "#!/bin/sh\necho >>\"$HOME/rounds\"\n", executable: true)
    }

    private func rounds() -> Int {
        ((try? String(contentsOf: home.appending(path: "rounds"), encoding: .utf8)) ?? "").filter { $0 == "\n" }.count
    }

    /// Until `condition` holds: a minute at most, for a Mac busy with other tests.
    private func until(_ condition: () throws -> Bool) rethrows {
        let end = Date.now.addingTimeInterval(60)
        while try !condition(), Date.now < end { usleep(20_000) }
    }

    @Test func aDayWithoutTheAppStopsItForGood() throws {
        defer { cleanUp() }
        try quickRounds()
        try ranBefore()
        try start()
        let engine = try #require(try record())
        // No Mac, no phone, for a day, and nothing waiting.
        try FileManager.default.setAttributes([.modificationDate: Date.now.addingTimeInterval(-25 * 3600)], ofItemAtPath: run.appending(path: "attached").path)
        try until { try record() == nil }
        #expect(try record() == nil, "stopped, so a restart doesn't bring it back")
        try until { !alive(engine.pid) }
        #expect(!alive(engine.pid))
    }

    /// What keeps the engine up without the app, as the watchdog and a quitting Mac ask it: a
    /// wake-up or a watch, a bot's or a session's, or a group chat at work. Nothing else does.
    @Test func whatKeepsItUp() throws {
        defer { cleanUp() }
        func waits() throws -> Bool {
            let process = Process()
            process.executableURL = URL(filePath: "/bin/sh")
            process.arguments = ["-c", RemoteServer.waiting(in: "\"$K\"")]
            process.environment = ["K": root.path, "PATH": "/usr/bin:/bin"]
            try process.run()
            process.waitUntilExit()
            return process.terminationStatus == 0
        }
        // Folders, but nothing in them: a bot with no wake-up, a chat not at work.
        for folder in ["bots/bot_a/wakes", "bots/bot_a/watches", "wakes/ses_1/wakes", "groups/grp_1"] {
            try FileManager.default.createDirectory(at: root.appending(path: folder), withIntermediateDirectories: true)
        }
        #expect(try !waits())
        for waiting in ["bots/bot_a/wakes/w1", "bots/bot_a/watches/abc/", "wakes/ses_1/wakes/w1", "wakes/ses_1/watches/abc/", "groups/grp_1/awake"] {
            let item = root.appending(path: waiting)
            if waiting.hasSuffix("/") { try FileManager.default.createDirectory(at: item, withIntermediateDirectories: true) } else { try write(item, "x") }
            #expect(try waits(), "\(waiting)")
            try FileManager.default.removeItem(at: item)
        }
    }

    @Test func aChatAtWorkKeepsItUp() throws {
        defer { cleanUp() }
        try quickRounds()
        let awake = root.appending(path: "groups/grp_1/awake")
        try FileManager.default.createDirectory(at: awake.deletingLastPathComponent(), withIntermediateDirectories: true)
        try write(awake, "x")
        try ranBefore()
        try start()
        let engine = try #require(try record())
        try FileManager.default.setAttributes([.modificationDate: Date.now.addingTimeInterval(-25 * 3600)], ofItemAtPath: run.appending(path: "attached").path)
        // Two of its checks (one each ten rounds) gone by.
        try until { rounds() >= 21 }
        #expect(rounds() >= 21)
        #expect(try record() == engine && alive(engine.pid), "a chat at work keeps it up")
    }

    @Test func twoAtOnceStartOne() async throws {
        defer { cleanUp() }
        try ranBefore()
        async let first = Task.detached { try self.start() }.value
        async let second = Task.detached { try self.start() }.value
        _ = try await (first, second)
        #expect(try started().count == 1)
        #expect(try record().map { alive($0.pid) } == true)
    }
}
