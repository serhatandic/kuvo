import Foundation
import Testing
@testable import KuvoLink

struct ComputerConnectionTests {
    private var host: PairedHost {
        PairedHost(address: LinkAddress(bytes: Data(repeating: 1, count: 16))!, key: Data(repeating: 2, count: 32), name: "Lab", kind: .machine)
    }

    @Test func choicesUseTheSameRequest() async throws {
        let request = LinkRequest(method: "POST", path: "kuvo/test", headers: ["idempotency-key": "save-once"], body: Data("{}".utf8))
        let ssh = ComputerSSH<ComputerConnectionMethod>(available: { _, alias in alias == "lab-ssh" }, send: { asked, _, _ in
            #expect(asked == request)
            return .ssh
        })
        let connection = ComputerConnection<ComputerConnectionMethod>(relay: { asked, _ in
            #expect(asked == request)
            return .relay
        }, ssh: ssh)
        #expect(try await connection.send(request, to: host, preference: .init(destination: "lab-ssh")) == .ssh)
        #expect(try await connection.send(request, to: host) == .relay)
        #expect(try await connection.send(request, to: host, preference: .init(method: .relay, destination: "lab-ssh")) == .relay)
        await #expect(throws: ComputerConnectionError.sshUnavailable) {
            try await connection.send(request, to: host, preference: .init(method: .ssh))
        }
    }

    @Test func aDroppedWriteIsReturnedToTheCaller() async throws {
        let ssh = ComputerSSH<Bool>(available: { _, _ in true }, send: { _, _, _ in throw LinkError.closed })
        let connection = ComputerConnection<Bool>(relay: { _, _ in true }, ssh: ssh)
        await #expect(throws: LinkError.closed) {
            try await connection.send(LinkRequest(method: "POST", path: "kuvo/test"), to: host)
        }
    }
}
