import Foundation
import Testing
@testable import KuvoHelperKit

@Test func rendezvousFormatIsCheckedBeforeReadingOrRewritingARecord() {
    for value in ["{}", #"{"formatVersion":0}"#, #"{"formatVersion":1}"#] {
        #expect(HostProof.supportsRendezvous(Data(value.utf8)))
    }
    for value in [#"{"formatVersion":2}"#, #"{"formatVersion":null}"#, #"{"formatVersion":true}"#, #"{"formatVersion":"1"}"#, #"{"formatVersion":"#] {
        #expect(!HostProof.supportsRendezvous(Data(value.utf8)))
    }
}

@MainActor @Test func helperHoldsAnUnknownHostFileBeforeOpeningItsListener() async throws {
    let home = FileManager.default.temporaryDirectory.appending(path: "kuvo-host-format-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: home.appending(path: "run"), withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: home) }
    let keychain = HostKeychain(testFile: home.appending(path: "unused.keychain").path, password: "fixture")
    let host = BoxHelperHost(home: home, keychain: keychain, prefix: "run.kuvo.code.format-test", app: "run.kuvo.code.format-test")
    let bytes = Data(#"{"formatVersion":2,"url":"http://127.0.0.1:1","password":"fixture"}"#.utf8)
    try bytes.write(to: host.file)
    do {
        try await host.start()
        host.stop()
        Issue.record("The helper opened its listener with an unknown rendezvous")
    } catch {
        #expect(error.localizedDescription.contains("format is unsupported"))
    }
    #expect(host.port == nil)
    #expect(try Data(contentsOf: host.file) == bytes)
}
