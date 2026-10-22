import Foundation
import KuvoLink
import Testing
@testable import KuvoCode

/// A phone opens a file an agent's reply or tool names on this Mac (a picture it made, a file it
/// shows) by its `file://` address: its bytes with its type, and words for one that's gone, too
/// big or not a file's address.
@Suite(.serialized) struct PhoneFilesTests {
    let folder: URL

    init() throws {
        TestEnvironment.prepare()
        folder = AppInfo.testRoot.appending(path: "PhoneFiles/\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    }

    private func ask(_ uri: String) async throws -> (status: Int, type: String?, body: Data) {
        var query = URLComponents()
        query.queryItems = [URLQueryItem(name: "uri", value: uri)]
        var status = 0
        var type: String?
        var body = Data()
        let request = LinkRequest(path: "kuvo/file?" + (query.percentEncodedQuery ?? ""))
        for try await part in PhoneLink.respond(to: request, link: nil, engine: URL(string: "http://127.0.0.1:9/")!, password: nil, agents: nil) {
            switch part {
            case .head(let code, let headers):
                status = code
                type = headers["content-type"]
            case .body(let chunk): body += chunk
            }
        }
        return (status, type, body)
    }

    @Test func aFileComesWithItsType() async throws {
        defer { try? FileManager.default.removeItem(at: folder) }
        let picture = folder.appending(path: "a chart #1.png")
        let bytes = Data((0..<100_000).map { UInt8($0 % 251) })
        try bytes.write(to: picture)
        let sent = try await ask(picture.absoluteString)
        #expect(sent.status == 200)
        #expect(sent.type == "image/png")
        #expect(sent.body == bytes)
    }

    @Test func oneGoneTooBigOrNotAFileSaysSo() async throws {
        defer { try? FileManager.default.removeItem(at: folder) }
        let gone = try await ask(folder.appending(path: "gone.png").absoluteString)
        #expect(gone.status == 404)
        #expect((try JSONSerialization.jsonObject(with: gone.body) as? [String: String])?["message"] != nil)
        #expect(try await ask(folder.absoluteString).status == 404, "a folder isn't a file")
        #expect(try await ask("https://example.com/a.png").status == 400)
        let big = folder.appending(path: "big.bin")
        FileManager.default.createFile(atPath: big.path, contents: nil)
        let handle = try FileHandle(forWritingTo: big)
        try handle.truncate(atOffset: UInt64(PhoneLink.fileLimit + 1))
        try handle.close()
        #expect(try await ask(big.absoluteString).status == 413)
    }
}
