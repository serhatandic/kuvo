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
    /// as on that machine. A letter from one of them reaches the conversation it's for, once,
    /// however often it comes; one from anyone else there, or too old, doesn't.
    @Test func aMachinesLetterReachesAConversationHereOnce() async throws {
        let prompts = await start()
        defer { done() }
        let before = AgentMail.shared.app
        AgentMail.shared.app = app
        defer { AgentMail.shared.app = before }

        await AgentMail.shared.mailed(["agents": ["at": Self.now, "list": [["session": "ses_faraway", "name": "API", "kind": "session", "project": "api"]]]], from: "lokum")
        let listed = try #require(AgentMail.shared.directory().first { $0.session == "ses_faraway" })
        #expect(listed.machine == "lokum" && listed.name == "API" && listed.project == "api")

        await AgentMail.shared.mailed(letter(from: "ses_imposter"), from: "lokum")
        await AgentMail.shared.mailed(letter(from: "ses_faraway", at: Self.now - 2 * 86_400_000), from: "lokum")
        #expect(prompts.all.isEmpty)

        let once = letter(from: "ses_faraway", id: "ltr_onceonly")
        await AgentMail.shared.mailed(once, from: "lokum")
        await AgentMail.shared.mailed(once, from: "lokum")
        #expect(prompts.all == [AgentMail.compose(from: "API", session: "ses_faraway", text: "The API's deployed.", waits: false)])
        // From another machine, naming lokum's agent: not one of its own.
        await AgentMail.shared.mailed(letter(from: "ses_faraway"), from: "elsewhere")
        #expect(prompts.all.count == 1)
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
