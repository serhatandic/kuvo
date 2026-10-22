import Foundation
import OpenCodeKit
import Testing
@testable import KuvoCode

/// "Also Apply to…": a change made on one computer's settings, made on another's too, and only
/// when you ask. A list gets what was added and loses what was taken out, keeping the other
/// computer's own; anything else is set as it is here; an integration's keys are named where the
/// other computer keeps them; what only a Mac can run stays off machines; and a change undone
/// before it's applied is nothing to apply. What's applied isn't a change of the other
/// computer's own, to offer back.
@MainActor
@Suite struct AlsoApplyTests {
    static func rule(_ resource: String, _ effect: String = "deny") -> JSONValue {
        .object(["action": .string("shell"), "resource": .string(resource), "effect": .string(effect)])
    }

    private func config(_ settings: [String: JSONValue]) throws -> EngineConfig {
        let folder = AppInfo.testRoot.appending(path: "AlsoApply/\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let file = folder.appending(path: "opencode.json")
        try JSONEncoder().encode(settings).write(to: file)
        return EngineConfig(file: file)
    }

    init() { TestEnvironment.prepare() }

    @Test func aRuleAddedHereJoinsTheOtherComputersOwn() throws {
        let here = try config(["permissions": .array([Self.rule("rm *")])])
        here.set(.array([Self.rule("rm *"), Self.rule("git push *")]), at: ["permissions"])
        let there: [String: JSONValue] = ["permissions": .array([Self.rule("docker *")])]
        let made = AlsoApply.translated(AlsoApply.net(here.changes), onto: there, to: "lab")
        #expect(made.count == 1)
        #expect(made[0].path == ["permissions"])
        #expect(made[0].value == .array([Self.rule("docker *"), Self.rule("git push *")]))
    }

    @Test func aRuleTakenOutHereGoesThereToo() throws {
        let here = try config(["permissions": .array([Self.rule("rm *"), Self.rule("git push *")])])
        here.set(.array([Self.rule("git push *")]), at: ["permissions"])
        let there: [String: JSONValue] = ["permissions": .array([Self.rule("rm *"), Self.rule("docker *")])]
        let made = AlsoApply.translated(AlsoApply.net(here.changes), onto: there, to: nil)
        #expect(made.first?.value == .array([Self.rule("docker *")]))
    }

    @Test func aChangeUndoneIsNothingToApply() throws {
        let here = try config(["snapshots": .bool(true)])
        here.set(.bool(false), at: ["snapshots"])
        here.set(.bool(true), at: ["snapshots"])
        #expect(here.changes.count == 2)
        #expect(AlsoApply.net(here.changes).isEmpty)
        here.set(.string("anthropic/sonnet"), at: ["model"])
        let made = AlsoApply.translated(AlsoApply.net(here.changes), onto: [:], to: "lab")
        #expect(made.map(\.path) == [["model"]])
    }

    /// An integration with a key, added here and applied to a machine: its settings name the key
    /// in that machine's own keys' folder, and the key goes with it.
    @Test func anIntegrationsKeyIsNamedWhereTheOtherKeepsIt() throws {
        let service = try #require(Integrations.all.first { !Integrations.keys(of: $0.way).isEmpty && !Integrations.macOnly.contains($0.id) })
        let here = try config([:])
        here.set(Integrations.settings(for: service), at: MCPSettings.path + [service.id])
        let made = AlsoApply.translated(AlsoApply.net(here.changes), onto: [:], to: "lab")
        let text = String(decoding: try JSONEncoder().encode(made.first?.value), as: UTF8.self)
        #expect(!text.contains(IntegrationKeys.folder.path))
        let names = AlsoApply.keys(in: made)
        #expect(!names.isEmpty)
        for name in names {
            #expect(text.contains(IntegrationKeys.reference(name, on: "lab").replacingOccurrences(of: "/", with: "\\/")) || text.contains(IntegrationKeys.reference(name, on: "lab")))
        }
        // And back to a Mac: its own folder.
        let back = AlsoApply.translated(AlsoApply.net(here.changes).map { EngineConfig.Change(id: $0.id, path: $0.path, before: $0.before, after: made.first?.value) }, onto: [:], to: nil)
        let backText = String(decoding: try JSONEncoder().encode(back.first?.value), as: UTF8.self)
        #expect(!backText.contains("/box/keys/"))
    }

    @Test func whatOnlyAMacRunsStaysOffMachines() throws {
        let id = try #require(Integrations.macOnly.first)
        let service = try #require(Integrations.service(id))
        let here = try config([:])
        here.set(Integrations.settings(for: service), at: MCPSettings.path + [id])
        #expect(AlsoApply.translated(AlsoApply.net(here.changes), onto: [:], to: "lab").isEmpty)
        #expect(AlsoApply.translated(AlsoApply.net(here.changes), onto: [:], to: nil).count == 1)
    }

    @Test func whatsAppliedIsntOfferedBack() throws {
        let there = try config([:])
        there.set([(["model"], .string("anthropic/sonnet"))], noted: false)
        #expect(there.value(at: ["model"]) == .string("anthropic/sonnet"))
        #expect(there.changes.isEmpty)
    }
}
