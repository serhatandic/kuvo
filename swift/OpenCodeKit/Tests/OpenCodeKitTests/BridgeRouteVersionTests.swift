import Foundation
import Testing
@testable import OpenCodeKit

@Test func aDirectBridgeRefusesAnUnknownReceiptVersion() async throws {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [BridgeReply.self]
    let session = URLSession(configuration: configuration)
    defer { session.invalidateAndCancel() }
    for version in ["legacy", "1", "2"] {
        let endpoint = ServerEndpoint(url: URL(string: "http://fixture-\(version).invalid")!, password: "fixture")
        let client = HTTPClient(endpoint: endpoint, session: session, boxRoutes: true)
        let request = try client.request("GET", "api/session", body: Empty?.none)
        #expect(request.value(forHTTPHeaderField: "X-Kuvo-Route-Version") == "1")
        if version == "2" {
            do {
                let _: Empty = try await client.send(request)
                Issue.record("Accepted a future reply")
            } catch let error as ServerError {
                #expect(error.status == 409)
            }
        } else {
            let _: Empty = try await client.send(request)
        }
    }
}

private final class BridgeReply: URLProtocol, @unchecked Sendable {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}

    override func startLoading() {
        guard let url = request.url, let host = url.host else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let version = host.split(separator: "-").last?.split(separator: ".").first.map(String.init) ?? ""

        let headers = version == "legacy" ? [:] : ["X-Kuvo-Route-Version": version]
        client?.urlProtocol(self, didReceive: HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: headers)!, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data("{}".utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
}
