import Foundation
import OpenCodeKit
import Testing
@testable import KuvoCode

/// Bots a machine makes itself, with this Mac closed (New Bot there, from a phone): the Mac gives
/// the machine what New Bot needs as it connects, and takes in what the machine made the next time
/// it does: each bot once, as its folder there describes it, the rest left as they are. What the
/// machine tells a new bot, from the kit's words, is what this Mac tells one.
@MainActor
@Suite(.serialized) struct MachineBotsTests {
    let app = AppModel()
    let botsFile = AppInfo.supportDirectory.appending(path: "bots.json")

    init() {
        TestEnvironment.prepare()
        try? FileManager.default.removeItem(at: botsFile)
        app.bots.app = app
    }

    private func done() {
        try? FileManager.default.removeItem(at: botsFile)
    }

    private func descriptor(_ id: String, name: String = "Herald", model: ModelRef? = ModelRef(id: "gpt-6.1-sol", providerID: "codex-cli"),
                            personality: String? = "preset.straight", project: String? = nil) throws -> Data {
        let descriptor = Bot.Descriptor(id: id, name: name, job: "", look: BotLook(color: .sky, shape: .round, face: .happy), personality: personality,
                                        model: model, project: project, created: Date(timeIntervalSince1970: 1_792_000_000), updated: Date(timeIntervalSince1970: 1_792_000_000))
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        return try encoder.encode(descriptor)
    }

    @Test func whatAMachineMadeIsTakenInOnce() throws {
        defer { done() }
        let folder = "/home/me/.kuvo-code/bots/bot_herald0001"
        let found = [
            Bots.Found(folder: folder, descriptor: try descriptor("bot_herald0001", project: "/home/me/app"), session: "ses_herald", via: "bridge"),
            // Not a bot's description, one in another's folder, one with no conversation: left out.
            Bots.Found(folder: "/home/me/.kuvo-code/bots/bot_broken0001", descriptor: Data("{}".utf8), session: "ses_x", via: ""),
            Bots.Found(folder: "/home/me/.kuvo-code/bots/bot_other00001", descriptor: try descriptor("bot_moved00001"), session: "ses_y", via: ""),
            Bots.Found(folder: "/home/me/.kuvo-code/bots/bot_nosession1", descriptor: try descriptor("bot_nosession1"), session: "", via: ""),
        ]
        let adopted = app.bots.adopt(found, on: "box")
        #expect(adopted.map(\.id) == ["bot_herald0001"])
        let bot = try #require(app.bots.bot("bot_herald0001"))
        #expect(bot.name == "Herald" && bot.machine == "box" && bot.folder == folder && bot.sessionID == "ses_herald")
        #expect(bot.model == ModelRef(id: "gpt-6.1-sol", providerID: "codex-cli") && bot.personality == "preset.straight")
        #expect(bot.agents == true, "it runs with the coding agents there")
        #expect(bot.project == RemoteFolder.folder("/home/me/app", on: "box"))
        #expect(app.workspace.botSessions.contains("ses_herald"))
        // Kept, and taken in only once.
        #expect(Bots().list.map(\.id) == ["bot_herald0001"])
        #expect(app.bots.adopt(found, on: "box").isEmpty)
        #expect(app.bots.list.count == 1)
    }

    /// The kit's words for a bot, filled in as the machine's bridge fills them in
    /// (`botInstructions` in kuvo-link.js), are this Mac's words for that bot.
    @Test func aMachinesBotIsToldWhatThisMacTellsOne() throws {
        defer { done() }
        let kit = app.bots.kit(for: "box")
        func filled(name: String, folder: String, personality: String?, project: Bool) -> String {
            var text = kit.instructions.replacingOccurrences(of: "⟦name⟧", with: name).replacingOccurrences(of: "⟦folder⟧", with: folder)
            text = personality.map { text.replacingOccurrences(of: "⟦personality⟧", with: $0) } ?? text.replacingOccurrences(of: "\n⟦personality⟧\n", with: "")
            if project { text = text.replacingOccurrences(of: "\n\n" + kit.identityNote, with: "\n\n" + kit.projectNote + "\n\n" + kit.identityNote) }
            return text
        }
        let straight = try #require(Personalities.shared.personality("preset.straight"))
        for (personality, project) in [(nil, false), ("preset.straight", false), (nil, true), ("preset.straight", true)] as [(String?, Bool)] {
            var bot = Bot(id: "bot_herald0001", name: "Herald", job: "", instructions: "", look: .random(), machine: "box",
                          project: project ? RemoteFolder.folder("/home/me/app", on: "box") : nil, folder: "/home/me/.kuvo-code/bots/bot_herald0001", sessionID: "s", created: .now)
            bot.personality = personality
            #expect(filled(name: "Herald", folder: bot.folder, personality: personality.map { _ in straight.instruction }, project: project) == Bots.instructions(for: bot),
                    "\(personality ?? "no personality"), \(project ? "a project" : "its own folder")")
        }
        #expect(kit.memory.replacingOccurrences(of: "⟦name⟧", with: "Herald") == Bots.blankMemory("Herald"))
        #expect(kit.templates.map(\.id) == BotTemplate.all.map(\.id))
    }
}
