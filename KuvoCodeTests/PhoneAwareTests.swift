import Foundation
import KuvoLink
import OpenCodeKit
import Testing
@testable import KuvoCode

/// The agent knows when you write from your phone: what a phone sends ends with a block saying
/// so (from this Mac, and in the same words from a machine's bridge with the Mac closed), which
/// your bubble never shows, marking the message as from your iPhone instead. There it shows you
/// a file with show_file, which puts it in the conversation; it's checked to be a file first.
@MainActor
@Suite(.serialized) struct PhoneAwareTests {
    let engine = StubEngine(port: 47995)
    let link = PhoneLink()

    init() {
        engine.on("POST", "/api/session/ses_1/prompt", json: [:])
    }

    private func send(_ text: String) async throws -> String? {
        let body = try JSONSerialization.data(withJSONObject: ["id": "msg_1", "text": text])
        let request = LinkRequest(method: "POST", path: "api/session/ses_1/prompt", headers: ["content-type": "application/json"], body: body)
        for try await _ in PhoneLink.respond(to: request, link: link, engine: URL(string: "http://127.0.0.1:47995/")!, password: nil, agents: nil) {}
        return engine.requests("POST", "/api/session/ses_1/prompt").last?.body["text"] as? String
    }

    @Test func whatAPhoneSendsSaysSoAndYourBubbleDoesnt() async throws {
        defer { engine.stop() }
        let sent = try #require(try await send("What's in the report?"))
        #expect(sent.hasPrefix("What's in the report?\n\n<from-phone>\n\(FromPhone.heading)\n"))
        #expect(sent.hasSuffix("</from-phone>") && sent.contains("show_file"))
        // Your bubble shows your words, from your iPhone.
        #expect(SentMessage.words(of: sent) == "What's in the report?")
        #expect(FromPhone.isFromPhone(sent))
        // Sent again, it's said once.
        #expect(FromPhone.adding(to: sent) == sent)
        // Words that only talk about the block keep it.
        #expect(!FromPhone.isFromPhone("What does <from-phone> mean?"))
    }

    /// A machine's bridge, with the Mac closed, says it in the same words.
    @Test func aMachineSaysItTheSameWay() throws {
        defer { engine.stop() }
        let script = try String(decoding: Bridge.script, as: UTF8.self)
        #expect(script.contains(#"["<from-phone>", "\#(FromPhone.heading)", "\#(FromPhone.advice)", "</from-phone>"]"#))
    }

    @Test func showFileChecksItsAFile() throws {
        defer { engine.stop() }
        let folder = AppInfo.testRoot.appending(path: "PhoneAware/\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let chart = folder.appending(path: "chart.png")
        try Data([1, 2, 3]).write(to: chart)
        func call(_ path: String) -> (text: String, failed: Bool) {
            let result = BrowserTools.showFileCall(["path": path])
            return (((result["content"] as? [[String: Any]])?.first?["text"] as? String) ?? "", result["isError"] as? Bool ?? true)
        }
        #expect(!call(chart.path).failed)
        #expect(call(chart.path).text.contains(chart.path))
        #expect(call(folder.appending(path: "gone.png").path).failed)
        #expect(call(folder.path).text.contains("is a folder"))
        #expect(call("chart.png").text == "Give the file's full path.")
        #expect(call("~/chart.png").failed, "the conversation shows it from the path as given")
    }
}
