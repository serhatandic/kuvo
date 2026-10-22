import CryptoKit
import Foundation
import KuvoLink
import Testing
@testable import KuvoCode

/// A phone removes another from this Mac, a lost one say, as you would in Settings › Phone: the
/// lost one can't come in again or change anything, the removal is kept (and goes to your
/// machines with the phones you've paired), and nobody removes the Mac. Phones paired before
/// pairings had an id keep none, as your machines have them.
@MainActor
@Suite(.serialized) struct PhoneRemovalTests {
    let folder: URL
    let mac = LinkKey(), mine = LinkKey(), lost = LinkKey()
    var file: URL { folder.appending(path: "phone-link.json") }

    init() throws {
        TestEnvironment.prepare()
        folder = AppInfo.testRoot.appending(path: "PhoneRemoval/\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        // As an older Kuvo Code kept it: no pairings, no removals.
        let phones = [(mine, "Mine"), (lost, "Lost")].map { #"{"key":"\#($0.0.publicBytes.base64URL)","name":"\#($0.1)","added":0}"# }
        try Data(#"{"phones":[\#(phones.joined(separator: ","))],"machines":{}}"#.utf8).write(to: file)
    }

    private func link() -> PhoneLink { PhoneLink(file: file, identity: (mac, RelaySecret())) }

    private func ask(_ link: PhoneLink, _ path: String, as phone: LinkKey, body: String? = nil) async throws -> (status: Int, json: [String: Any]?) {
        let request = LinkRequest(method: body == nil ? "GET" : "POST", path: path, headers: ["content-type": "application/json"], body: Data((body ?? "").utf8))
        var status = 0
        var data = Data()
        for try await part in PhoneLink.respond(to: request, link: link, engine: URL(string: "http://127.0.0.1:9/")!, password: nil, agents: nil, asking: phone.publicBytes) {
            switch part {
            case .head(let code, _): status = code
            case .body(let chunk): data += chunk
            }
        }
        return (status, try? JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    private func removal(_ key: LinkKey, pairing: String? = nil) -> String {
        #"{"phones":[{"key":"\#(key.publicBytes.base64URL)"\#(pairing.map { #","pairing":"\#($0)""# } ?? "")}]}"#
    }

    @Test func aPhoneRemovesALostOneForGood() async throws {
        defer { try? FileManager.default.removeItem(at: folder) }
        let link = link()
        #expect(link.phones.count == 2 && link.phones.allSatisfy { $0.pairing.isEmpty }, "as your machines have them")

        let listed = try #require(try await ask(link, "kuvo/phones", as: mine).json?["phones"] as? [[String: Any]])
        #expect(listed.map { $0["name"] as? String } == ["Mine", "Lost"])
        #expect(listed.map { $0["you"] as? Bool } == [true, false], "it knows itself")
        let pairing = try #require(listed[1]["pairing"] as? String)
        #expect(try await ask(link, "kuvo/phones/remove", as: mine, body: #"{"phones":[{"key":"not a phone"}]}"#).status == 400)

        #expect(try await ask(link, "kuvo/phones/remove", as: mine, body: removal(mac)).status == 400, "never the Mac")
        #expect(try await ask(link, "kuvo/phones/remove", as: mine, body: removal(lost, pairing: pairing)).status == 200)
        #expect(link.phones.map(\.name) == ["Mine"])
        #expect(try await ask(link, "kuvo/phones/remove", as: lost, body: removal(mine)).status == 400, "a removed phone changes nothing")
        #expect(link.phones.map(\.name) == ["Mine"])

        // Kept, for a restart and for your machines, which keep it out too.
        let again = self.link()
        #expect(again.phones.map(\.name) == ["Mine"])
        #expect(again.paired.removed == [.init(key: lost.publicBytes.base64URL, pairing: pairing)])
    }

    @Test func aPhoneRemovesItself() async throws {
        defer { try? FileManager.default.removeItem(at: folder) }
        let link = link()
        #expect(try await ask(link, "kuvo/phones/remove", as: mine, body: removal(mine)).status == 200)
        #expect(link.phones.map(\.name) == ["Lost"])
    }
}
