import Foundation
import Testing
@testable import KuvoLink

/// `Vectors/link.json`, which the JavaScript bridge checks against too.
@Suite struct VectorTests {
    let vectors = try! Vectors.load("link.json")

    private func list(_ key: String) -> [[String: Any]] { vectors[key] as! [[String: Any]] }
    private func bytes(_ item: [String: Any], _ key: String) -> Data { Data(hex: item[key] as! String)! }

    @Test func addresses() {
        for item in list("addresses") {
            #expect(RelaySecret(bytes: bytes(item, "secret"))!.address.bytes == bytes(item, "address"))
        }
    }

    @Test func relayMessages() throws {
        for item in list("relay") {
            let data = bytes(item, "bytes")
            switch item["name"] as! String {
            case "hello":
                #expect(RelayMessage.hello(RelaySecret(bytes: data.dropFirst())!).data == data)
            case "send":
                let to = LinkAddress(bytes: bytes(item, "to"))!
                #expect(RelayMessage.send(to: to, payload: bytes(item, "payload")).data == data)
            case "welcome":
                #expect(try RelayMessage(fromRelay: data) == .welcome(LinkAddress(bytes: bytes(item, "address"))!))
            case "deliver":
                #expect(try RelayMessage(fromRelay: data) == .deliver(from: LinkAddress(bytes: bytes(item, "address"))!, payload: bytes(item, "payload")))
            case "unreachable":
                #expect(try RelayMessage(fromRelay: data) == .unreachable(LinkAddress(bytes: bytes(item, "address"))!))
            case "pushed":
                #expect(try RelayMessage(fromRelay: data) == .pushed(to: LinkAddress(bytes: bytes(item, "address"))!, status: .notAllowed))
            default:
                Issue.record("Unknown vector \(item["name"]!)")
            }
        }
    }

    @Test func frames() throws {
        for item in list("frames") {
            let frame = LinkFrame(LinkFrame.Kind(rawValue: UInt8(item["kind"] as! Int))!, session: UInt32(item["session"] as! Int), body: bytes(item, "body"))
            #expect(frame.data == bytes(item, "bytes"))
            #expect(try LinkFrame(bytes(item, "bytes")) == frame)
        }
    }

    @Test func routes() {
        for item in list("routes") {
            #expect(EngineRoutes.allows(item["path"] as! String, method: item["method"] as? String ?? "GET") == item["allowed"] as! Bool, "\(item["path"]!) \(item["method"] ?? "GET")")
        }
    }

    @Test func agentRoutes() {
        for item in list("agentRoutes") {
            #expect(EngineRoutes.allowsAgents(item["path"] as! String, method: item["method"] as? String ?? "GET") == item["allowed"] as! Bool, "\(item["path"]!) \(item["method"] ?? "GET")")
        }
    }

    @Test func signInReturns() {
        for item in list("signInReturns") {
            #expect(EngineRoutes.signInReturn(item["url"] as! String)?.absoluteString == item["loads"] as? String, "\(item["url"]!)")
        }
    }

    @Test func sockets() {
        for item in list("sockets") {
            #expect(EngineRoutes.allowsSocket(item["path"] as! String) == item["allowed"] as! Bool, "\(item["path"]!)")
        }
    }

    @Test func messages() throws {
        for item in list("messages") {
            let head = try JSONDecoder().decode(LinkMessage.Head.self, from: JSONSerialization.data(withJSONObject: item["head"]!))
            let message = LinkMessage(head, body: bytes(item, "body"))
            #expect(message.data == bytes(item, "bytes"))
            #expect(try LinkMessage(bytes(item, "bytes")) == message)
        }
    }
}
