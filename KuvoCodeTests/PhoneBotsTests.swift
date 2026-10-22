import Foundation
import KuvoLink
import OpenCodeKit
import Testing
@testable import KuvoCode

/// Making bots and group chats from a phone, as New Bot and New Group Chat make them: what New
/// Bot offers (templates, personalities, looks, the model it starts with in each place, your
/// projects); a bot from scratch writes first, one from a template waits with its job for the
/// phone's composer; a chat of two of them; and what's missing or unknown said in words. The
/// engine is stubbed.
@MainActor
@Suite(.serialized) struct PhoneBotsTests {
    let engine = StubEngine(port: 47996)
    let app = AppModel()
    let link = PhoneLink()
    let botsFile = AppInfo.supportDirectory.appending(path: "bots.json")
    let groupsFile = AppInfo.supportDirectory.appending(path: "groups.json")

    init() {
        app.bots.app = app
        app.wakeUps.app = app
        app.groups.app = app
        app.workspace.api = engine.api
        link.bots = app.bots
        link.groups = app.chats
        link.workspace = app.workspace
        engine.list([])
        engine.on("PATCH", "/api/session/*", json: [:])
        engine.on("PUT", "/api/experimental/session/*", json: [:])
        engine.on("POST", "/api/session/*", json: [:])
        var made = 0
        engine.on("POST", "/api/session") { request in
            made += 1
            return StubWeb.json(["data": StubEngine.session("ses_bot\(made)", title: request.body["title"] as? String ?? "",
                                                             directory: (request.body["location"] as? [String: Any])?["directory"] as? String ?? "")])
        }
    }

    private func done() {
        engine.stop()
        for bot in app.bots.list { try? FileManager.default.removeItem(atPath: bot.folder) }
        try? FileManager.default.removeItem(at: botsFile)
        try? FileManager.default.removeItem(at: groupsFile)
    }

    private func ask(_ path: String, method: String = "GET", body: [String: Any]? = nil) async throws -> (status: Int, json: [String: Any]?) {
        let data = try body.map { try JSONSerialization.data(withJSONObject: $0) } ?? Data()
        let request = LinkRequest(method: method, path: path, headers: ["content-type": "application/json"], body: data)
        var status = 0
        var answer = Data()
        for try await part in PhoneLink.respond(to: request, link: link, engine: URL(string: "http://127.0.0.1:47996/")!, password: nil, agents: nil) {
            switch part {
            case .head(let code, _): status = code
            case .body(let chunk): answer += chunk
            }
        }
        return (status, try? JSONSerialization.jsonObject(with: answer) as? [String: Any])
    }

    private func message(_ answer: (status: Int, json: [String: Any]?)) -> String? { answer.json?["message"] as? String }

    @Test func whatNewBotOffers() async throws {
        defer { done() }
        let offered = try #require(try await ask("kuvo/bots/templates").json)
        let templates = try #require(offered["templates"] as? [[String: Any]])
        #expect(templates.map { $0["id"] as? String } == BotTemplate.all.map(\.id))
        let ci = try #require(templates.first { $0["id"] as? String == "ci" })
        #expect(ci["wantsProject"] as? Bool == true)
        #expect(ci["personality"] as? String == "preset.perfectionist")
        #expect((ci["look"] as? [String: String])?["shape"] == "squircle")
        let personalities = try #require(offered["personalities"] as? [[String: Any]])
        #expect(personalities.first?["id"] as? String == "preset.straight" && personalities.first?["custom"] as? Bool == false)
        let looks = try #require(offered["looks"] as? [String: [String]])
        #expect(looks["colors"]?.contains("mint") == true && looks["shapes"]?.contains("bloom") == true && looks["faces"]?.contains("wink") == true)
        // This Mac is always a place to make one, its model or not known yet.
        #expect((offered["startingModel"] as? [String: Any])?.keys.contains("mac") == true)
        #expect(offered["projects"] is [[String: Any]])
    }

    @Test func aBotFromScratchWritesFirst() async throws {
        defer { done() }
        await app.workspace.refresh()
        let made = try await ask("kuvo/bots", method: "POST", body: ["name": "Inbox Helper", "look": ["color": "mint", "shape": "round", "face": "calm"]])
        #expect(made.status == 201)
        let bot = try #require(made.json?["bot"] as? [String: Any])
        let id = try #require(bot["id"] as? String)
        #expect(id.hasPrefix("bot_") && bot["name"] as? String == "Inbox Helper" && bot["session"] as? String == "ses_bot1")
        #expect(made.json?["firstMessage"] is NSNull, "no job for the composer: it writes first")
        #expect(app.bots.bot(id)?.look == BotLook(color: .mint, shape: .round, face: .calm))
        // It writes first, as a new bot does.
        for _ in 0..<100 where engine.requests("POST", "/api/session/ses_bot1/synthetic").isEmpty { try await Task.sleep(for: .milliseconds(20)) }
        #expect(engine.requests("POST", "/api/session/ses_bot1/synthetic").count == 1)
        // Listed by its id, as group chats name their members.
        let listed = try #require(try await ask("kuvo/bots").json?["bots"] as? [[String: Any]])
        #expect(listed.map { $0["id"] as? String } == [id])
    }

    @Test func oneFromATemplateWaitsWithItsJob() async throws {
        defer { done() }
        await app.workspace.refresh()
        let made = try await ask("kuvo/bots", method: "POST", body: ["name": "Herald", "template": "news"])
        #expect(made.status == 201)
        let news = try #require(BotTemplate.all.first { $0.id == "news" })
        #expect(made.json?["firstMessage"] as? String == BotTemplate.firstMessage(job: news.job, instructions: news.instructions))
        let bot = try #require(app.bots.list.first)
        #expect(bot.look == news.look && bot.personality == news.personality, "the template's look and personality")
        try await Task.sleep(for: .milliseconds(200))
        #expect(engine.requests("POST", "/api/session/ses_bot1/synthetic").isEmpty, "it waits for you")
    }

    @Test func whatsMissingOrUnknownIsSaid() async throws {
        defer { done() }
        await app.workspace.refresh()
        for (body, words) in [
            (["look": ["color": "mint", "shape": "round", "face": "calm"]], "A bot needs a name."),
            (["name": "X", "template": "nope"], "There’s no template “nope”."),
            (["name": "X", "personality": "custom.gone"], "There’s no personality “custom.gone”."),
            (["name": "X", "machine": "box"], "There’s no machine “box”."),
            (["name": "X", "project": "/nowhere"], "That project isn’t in Kuvo Code."),
        ] as [([String: Any], String)] {
            let answer = try await ask("kuvo/bots", method: "POST", body: body)
            #expect(answer.status == 400 && message(answer) == words, "\(body)")
        }
        #expect(app.bots.list.isEmpty, "nothing made")
    }

    @Test func aGroupChatOfTwo() async throws {
        defer { done() }
        await app.workspace.refresh()
        let ada = try await app.bots.create(name: "Ada", look: .random(), model: nil, machine: nil, project: nil)
        let ben = try await app.bots.create(name: "Ben", look: .random(), model: nil, machine: nil, project: nil)
        let made = try await ask("kuvo/groups", method: "POST", body: ["name": "Release", "members": [ada.id, ben.id, ada.id], "purpose": "  Ship 1.0  "])
        #expect(made.status == 201)
        let group = try #require(made.json?["group"] as? [String: Any])
        #expect(group["name"] as? String == "Release" && group["purpose"] as? String == "Ship 1.0")
        #expect((group["members"] as? [[String: Any]])?.map { $0["id"] as? String } == [ada.id, ben.id])
        #expect(app.groups.list.map(\.name) == ["Release"])
        // Listed as the phone lists the rest.
        let listed = try #require(try await ask("kuvo/groups").json?["groups"] as? [[String: Any]])
        #expect(listed.map { $0["id"] as? String } == [group["id"] as? String])

        let one = try await ask("kuvo/groups", method: "POST", body: ["name": "Solo", "members": [ada.id]])
        #expect(one.status == 400 && message(one) == "A group chat needs at least two bots.")
        let unnamed = try await ask("kuvo/groups", method: "POST", body: ["name": " ", "members": [ada.id, ben.id]])
        #expect(unnamed.status == 400)
        let unknown = try await ask("kuvo/groups", method: "POST", body: ["name": "Who", "members": [ada.id, "bot_gone000001"]])
        #expect(unknown.status == 404 && message(unknown) == "There’s no bot “bot_gone000001”.")
        #expect(app.groups.list.count == 1)
    }
}
