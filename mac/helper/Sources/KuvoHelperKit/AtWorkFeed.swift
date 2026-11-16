#if os(macOS)
import Foundation
import os

nonisolated private let log = Logger(subsystem: "run.kuvo.code", category: "menu-bar")

/// Kuvo on this Mac's own server, as the helper reaches it: the port its `run/serve.json` names, and
/// the password the helper started it with (only a server the helper started answers to it, so a
/// record a crash left, naming a port someone else took, is never read from).
public struct BoxServer: Sendable {
    public let port: Int
    public let password: String

    /// The server `run` records, if it's the one started with `password`.
    public static func recorded(in run: URL, password: String?) -> BoxServer? {
        guard let password,
              let data = try? Data(contentsOf: run.appending(path: "serve.json")),
              let record = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let port = record["port"] as? Int, (1...65535).contains(port),
              record["password"] as? String == password else { return nil }
        return BoxServer(port: port, password: password)
    }

    public func request(_ route: String, method: String = "GET") -> URLRequest {
        var request = URLRequest(url: URL(string: "http://127.0.0.1:\(port)/\(route)")!)
        request.httpMethod = method
        request.setValue("Basic " + Data("opencode:\(password)".utf8).base64EncodedString(), forHTTPHeaderField: "Authorization")
        request.timeoutInterval = 60 * 60 * 24
        return request
    }
}

/// What's at work, followed as the box says it (`kuvo/box/at-work/events`, a line now and one as
/// it changes), reached again whenever the line drops (the server starting again, a new password),
/// a little later each time in a row, up to ten seconds.
@MainActor
public final class AtWorkFeed {
    private let server: () -> BoxServer?
    private let told: (AtWork?) -> Void
    private var following: Task<Void, Never>?
    private let session: URLSession

    /// `server`: where the box answers now (nil while it doesn't); `told`: each answer, and nil
    /// once it stops answering.
    public init(server: @escaping () -> BoxServer?, told: @escaping (AtWork?) -> Void) {
        self.server = server
        self.told = told
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 60 * 60 * 24
        configuration.connectionProxyDictionary = [:]
        session = URLSession(configuration: configuration)
    }

    public func start() {
        guard following == nil else { return }
        following = Task { [weak self] in await self?.follow() }
    }

    public func stop() {
        following?.cancel()
        following = nil
    }

    private func follow() async {
        var wait = 0.5
        while !Task.isCancelled {
            if let server = server() {
                do {
                    let (bytes, response) = try await session.bytes(for: server.request("kuvo/box/at-work/events"))
                    guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw URLError(.badServerResponse) }
                    for try await line in bytes.lines {
                        guard let atWork = AtWork.read(Data(line.utf8)) else { continue }
                        wait = 0.5
                        told(atWork)
                    }
                } catch {
                    if Task.isCancelled { return }
                    log.info("What's at work couldn't be followed: \(error.localizedDescription, privacy: .public)")
                }
                told(nil)
            }
            try? await Task.sleep(for: .seconds(wait))
            wait = min(wait * 2, 10)
        }
    }

    /// Where Kuvo's desktop app is that last opened with this Mac's Kuvo, as it told the box
    /// (`kuvo/box/app`'s `desktop`); nil where it never said, or the box doesn't answer.
    public func registeredDesktop() async -> URL? {
        guard let server = server() else { return nil }
        var request = server.request("kuvo/box/app")
        // (Its own server, on this Mac: it answers at once, or Open Kuvo goes on without it.)
        request.timeoutInterval = 1.5
        guard let (data, response) = try? await session.data(for: request), (response as? HTTPURLResponse)?.statusCode == 200,
              let said = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let desktop = said["desktop"] as? String, desktop.hasPrefix("/") else { return nil }
        return URL(filePath: desktop)
    }

    /// Asks the box to stop Kuvo on this Mac until it next starts (`POST kuvo/box/stop`). Nil once
    /// it's under way; else why not, in the box's words where it gave any.
    public func askToStop() async -> String? {
        guard let server = server() else { return "Kuvo on this Mac isn’t answering yet. Try again in a moment." }
        var request = server.request("kuvo/box/stop", method: "POST")
        request.timeoutInterval = 30
        request.httpBody = Data("{}".utf8)
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        do {
            let (data, response) = try await session.data(for: request)
            if (response as? HTTPURLResponse)?.statusCode == 202 { return nil }
            let said = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["message"] as? String
            return said ?? "Kuvo on this Mac didn’t stop."
        } catch {
            return "Kuvo on this Mac didn’t answer: \(error.localizedDescription)"
        }
    }
}
#endif
