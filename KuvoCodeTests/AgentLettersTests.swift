import Foundation
import OpenCodeKit
import Testing
@testable import KuvoCode

/// Agents on a machine this Mac isn't connected to, by mail through the relay (PROTOCOL.md,
/// "Peers"): the list of who's there, and their letters to conversations here. The engine is
/// stubbed; the relay isn't connected, so what this Mac would mail back goes nowhere.
@MainActor
@Suite(.serialized) struct AgentLettersTests {
    static let engine = "http://127.0.0.1:47994"
    let app = AppModel()
    let here = "ses_onthemac"

    private static func session(_ id: String) -> [String: Any] {
        [
            "id": id, "projectID": "p", "cost": 0, "title": "Release notes",
            "tokens": ["input": 0, "output": 0, "reasoning": 0, "cache": ["read": 0, "write": 0]],
            "time": ["created": 1, "updated": 2], "location": ["directory": "/tmp/kuvo-tests-project"],
            "model": ["id": "luna", "providerID": "openai"], "agent": "build", "metadata": [:] as [String: Any],
        ]
    }

    /// The prompts the engine was sent.
    nonisolated final class Prompts: @unchecked Sendable {
        private let lock = NSLock()
        private var texts: [String] = []
        func add(_ text: String) { lock.withLock { texts.append(text) } }
        var all: [String] { lock.withLock { texts } }
    }

    private func start() async -> Prompts {
        TestEnvironment.prepare()
        try? FileManager.default.removeItem(at: AgentMail.seenFile)
        StubWeb.start(hosts: ["127.0.0.1"])
        let listed = StubWeb.json(["data": [Self.session(here)]])
        StubWeb.route(Self.engine + "/api/session") { _ in listed }
        StubWeb.route(Self.engine + "/api/session/active") { _ in StubWeb.json(["data": [:]]) }
        let one = StubWeb.json(["data": Self.session(here)])
        StubWeb.route(Self.engine + "/api/session/\(here)") { _ in one }
        StubWeb.route(Self.engine + "/api/session/\(here)/message") { _ in StubWeb.json(["data": []]) }
        let prompts = Prompts()
        StubWeb.handle(Self.engine + "/api/session/\(here)/prompt") { _, body in
            prompts.add((try? JSONSerialization.jsonObject(with: body) as? [String: Any])?["text"] as? String ?? "")
            return StubWeb.json([:])
        }
        app.workspace.api = OpenCodeAPI(endpoint: ServerEndpoint(url: URL(string: Self.engine)!, password: "test"))
        await app.workspace.refresh()
        return prompts
    }

    private func done() {
        StubWeb.stop()
        try? FileManager.default.removeItem(at: AgentMail.seenFile)
    }

    private static var now: Double { (Date.now.timeIntervalSince1970 * 1000).rounded() }

    private func letter(from: String, id: String = "ltr_" + UUID().uuidString.prefix(8), at: Double = now, text: String = "The API's deployed.") -> [String: Any] {
        ["letter": ["id": id, "from": from, "name": "API", "to": here, "text": text, "wait": false, "at": at], "at": at]
    }

    /// A machine whose bridge talks to your other computers tells this Mac its agents: listed here
    /// as on that machine. A letter from one of them reaches the conversation it's for, by the
    /// name that machine gave it, once, however often it comes. One from anything else there, or
    /// naming another machine's agent, comes from "an agent on" it, with no address to answer;
    /// one too old, or for a conversation this Mac doesn't list, doesn't come.
    @Test func aMachinesLetterReachesAConversationHereOnce() async throws {
        let prompts = await start()
        defer { done() }
        let before = AgentMail.shared.app
        AgentMail.shared.app = app
        defer { AgentMail.shared.app = before }

        await AgentMail.shared.mailed(["agents": ["at": Self.now, "list": [["session": "ses_faraway", "name": "API", "kind": "session", "project": "api"]]]], from: "peerbox")
        let listed = try #require(AgentMail.shared.directory().first { $0.session == "ses_faraway" })
        #expect(listed.machine == "peerbox" && listed.name == "API" && listed.project == "api")

        await AgentMail.shared.mailed(letter(from: "ses_faraway", at: Self.now - 2 * 86_400_000), from: "peerbox")
        var other = letter(from: "ses_faraway")
        other["letter"] = (other["letter"] as? [String: Any])?.merging(["to": "ses_notlisted"]) { $1 }
        await AgentMail.shared.mailed(other, from: "peerbox")
        #expect(prompts.all.isEmpty)

        let once = letter(from: "ses_faraway", id: "ltr_onceonly")
        await AgentMail.shared.mailed(once, from: "peerbox")
        await AgentMail.shared.mailed(once, from: "peerbox")
        #expect(prompts.all == [AgentMail.compose(from: "API", session: "ses_faraway", text: "The API's deployed.", waits: false)])

        await AgentMail.shared.mailed(letter(from: "ses_imposter"), from: "peerbox")
        await AgentMail.shared.mailed(letter(from: "ses_faraway"), from: "elsewhere")
        #expect(Array(prompts.all.dropFirst()) == [AgentMail.compose(from: "an agent on peerbox", session: "unknown", text: "The API's deployed.", waits: false),
                                                  AgentMail.compose(from: "an agent on elsewhere", session: "unknown", text: "The API's deployed.", waits: false)])
        AgentMail.shared.machineRemoved("peerbox")
    }

    /// A message for an agent on a machine this Mac isn't connected to, with no way to mail it
    /// (the relay isn't connected): it waits for that machine, and never goes to this Mac's own
    /// engine, which would say there's no such session.
    @Test func aMessageForAnAwayMachinesAgentNeverGoesToTheMacsEngine() async throws {
        let prompts = await start()
        defer { done() }
        let before = AgentMail.shared.app
        AgentMail.shared.app = app
        defer { AgentMail.shared.app = before }
        let asked = Prompts()
        StubWeb.handle(Self.engine + "/api/session/ses_faraway/prompt") { _, _ in
            asked.add("ses_faraway")
            return StubWeb.json([:])
        }
        await AgentMail.shared.mailed(["agents": ["at": Self.now, "list": [["session": "ses_faraway", "name": "API", "kind": "session"]]]], from: "awaybox")
        defer { AgentMail.shared.machineRemoved("awaybox") }
        let sent = await AgentMail.shared.send(from: here, name: "Release notes", to: "API", text: "Is it deployed?", waits: false)
        #expect(!sent.isError, "\(sent.text)")
        #expect(sent.text.contains("awaybox"), "\(sent.text)")
        #expect(asked.all.isEmpty && prompts.all.isEmpty)
        let read = await AgentMail.shared.read("API", query: nil, caller: here)
        #expect(read.isError && read.text.contains("isn't connected"), "\(read.text)")
    }

    /// A letter's words cut to fit one mail: whole characters, within the bytes.
    @Test func aLettersWordsCutToFit() {
        let words = String(repeating: "é👍🏽a", count: 5000)
        let cut = AgentMail.prefix(words, bytes: 1000)
        #expect(cut.utf8.count <= 1000 && cut.utf8.count > 980)
        #expect(words.hasPrefix(cut))
        #expect(AgentMail.prefix("short", bytes: 1000) == "short")
    }
}
