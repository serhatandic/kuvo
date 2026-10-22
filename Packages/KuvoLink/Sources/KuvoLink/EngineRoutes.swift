import Foundation

/// What a phone may ask an engine for, and passing it on: the Mac's side of what a machine's
/// bridge does in `kuvo-link.js` (the two lists are checked against `Vectors/link.json`).
public enum EngineRoutes {
    /// A paired phone is yours: the engine's whole API, as the app has it (sessions, files,
    /// terminals, commands, accounts and keys, settings, MCP servers). Only a path that isn't what
    /// it seems (`staysInProject`) is refused.
    public static func allows(_ path: String, method: String = "GET") -> Bool {
        staysInProject(path) && path.prefixMatch(of: #/api(/|\?|$)/#) != nil
    }

    /// The same of a coding agents' bridge, but for the two routes only the app wires things with:
    /// where the engine is, and its group chats' own steps (moves), which a phone does through
    /// `kuvo/groups`.
    public static func allowsAgents(_ path: String, method: String = "GET") -> Bool {
        allows(path, method: method) && routed(path).prefixMatch(of: #/api/kuvo/(engine|groups)(/|\?|$)/#) == nil
    }

    /// `path` as the agents bridge routes it: split into parts, without empty ones or "." ones
    /// (which its URL parser drops, "%2e" too), each decoded ("%75pdate" is "update"). So
    /// "integration/x//update", ".../x/./update" and ".../x/%75pdate" are "integration/x/update".
    static func routed(_ path: String) -> String {
        let query = path.firstIndex(of: "?")
        let parts = path[..<(query ?? path.endIndex)].split(separator: "/")
            .filter { $0 != "." && $0.lowercased() != "%2e" }
            .map { $0.removingPercentEncoding ?? String($0) }
        return parts.joined(separator: "/") + (query.map { String(path[$0...]) } ?? "")
    }

    /// Where a sign-in's page sent the browser back to the computer, for the computer to load
    /// (`kuvo/sign-in/return`): only its own address (127.0.0.1, localhost, [::1]) over http, at a
    /// port, with what the page sent back (a query), written plainly. What's loaded is the address
    /// without its fragment, which a browser doesn't send; nil for anything else.
    public static func signInReturn(_ url: String) -> URL? {
        guard url.utf8.count <= 8192, url.unicodeScalars.allSatisfy({ ("!"..."~").contains($0) && $0 != "\\" }),
              let match = url.wholeMatch(of: #/(http://(?:127\.0\.0\.1|localhost|\[::1\]):([1-9][0-9]{0,4})(?:/[^?#]*)?\?[^#]+)(?:#.*)?/#),
              let port = Int(match.2), port <= 65535 else { return nil }
        return URL(string: String(match.1))
    }

    /// The sockets a phone may open: a terminal's.
    public static func allowsSocket(_ path: String) -> Bool {
        staysInProject(path) && path.prefixMatch(of: #/api/pty/[^/?]+/connect(\?|$)/#) != nil
    }

    /// No way out of the project, written plainly or percent-encoded ("%2E%2E"): the engine
    /// decodes it, and lists a folder above the project if asked. Nor a fragment ("#…"), which
    /// isn't sent: ".../update#x" would be checked as itself and reach ".../update". Nor tabs,
    /// line breaks or other control characters, which URL parsers drop (".\t." is ".." to them),
    /// nor a space at either end, which they drop too (".../update " is ".../update" to them).
    /// Nor a sign they split on (`/`, `?`, `#`, `.`, `\`, `%`) joined into one character with a
    /// mark after it, which these checks, made on characters, wouldn't see as that sign
    /// (".../update#\u{301}" is ".../update" to them).
    static func staysInProject(_ path: String) -> Bool {
        guard !path.unicodeScalars.contains(where: { $0.value < 0x20 || $0.value == 0x7f }),
              path.unicodeScalars.first != " ", path.unicodeScalars.last != " ",
              !joinsASign(path), let decoded = path.removingPercentEncoding, !joinsASign(decoded) else { return false }
        // As bytes, as parsers read them, not characters ("..\u{200D}" is ".." to them).
        return !has("..", in: decoded) && !has("\\", in: decoded) && !has("..", in: path) && !has("#", in: path)
    }

    /// A sign other than a letter or digit, joined into one character with what follows it.
    private static func joinsASign(_ text: String) -> Bool {
        text.contains { $0.unicodeScalars.count > 1 && $0.unicodeScalars.contains { $0.isASCII && !$0.properties.isAlphabetic && !("0"..."9").contains($0) } }
    }

    private static func has(_ part: String, in text: String) -> Bool {
        let bytes = Array(text.utf8), wanted = Array(part.utf8)
        guard bytes.count >= wanted.count else { return false }
        return (0...(bytes.count - wanted.count)).contains { Array(bytes[$0..<($0 + wanted.count)]) == wanted }
    }

    static let requestHeaders = ["content-type", "accept", "x-opencode-directory", "last-event-id"]
    static let responseHeaders = ["content-type", "cache-control"]

    /// Opens the socket `request` names on the engine at `base` (its password added here): text
    /// from `input` goes to it, and what it sends comes back, `.open` first.
    public static func connect(_ request: LinkRequest, to base: URL, password: String?, input: AsyncStream<Data>) -> AsyncThrowingStream<LinkSocketEvent, Error> {
        AsyncThrowingStream { continuation in
            guard allowsSocket(request.path), let url = URL(string: request.path, relativeTo: base),
                  var components = URLComponents(url: url.absoluteURL, resolvingAgainstBaseURL: false)
            else { return continuation.finish(throwing: URLError(.badURL)) }
            components.scheme = components.scheme == "https" ? "wss" : "ws"
            guard let socketURL = components.url else { return continuation.finish(throwing: URLError(.badURL)) }
            var outgoing = URLRequest(url: socketURL)
            if let directory = request.headers["x-opencode-directory"] { outgoing.setValue(directory, forHTTPHeaderField: "x-opencode-directory") }
            if let password { outgoing.setValue("Basic " + Data("opencode:\(password)".utf8).base64EncodedString(), forHTTPHeaderField: "Authorization") }
            // A session of its own, to hear when the socket's open.
            let watcher = SocketWatcher()
            let session = URLSession(configuration: .ephemeral, delegate: watcher, delegateQueue: nil)
            let socket = session.webSocketTask(with: outgoing)
            socket.resume()
            let reading = Task {
                do {
                    try await watcher.opened()
                    continuation.yield(.open)
                    while true {
                        switch try await socket.receive() {
                        case .string(let text): continuation.yield(.text(Data(text.utf8)))
                        case .data(let data): continuation.yield(.binary(data))
                        @unknown default: break
                        }
                    }
                } catch {
                    // Closed by the engine (the shell exited): the end, not a failure.
                    if socket.closeCode != .invalid { continuation.finish() } else { continuation.finish(throwing: error) }
                }
            }
            let writing = Task {
                for await text in input {
                    try? await socket.send(.string(String(decoding: text, as: UTF8.self)))
                }
                socket.cancel(with: .normalClosure, reason: nil)
            }
            continuation.onTermination = { _ in
                reading.cancel()
                writing.cancel()
                socket.cancel(with: .goingAway, reason: nil)
                session.finishTasksAndInvalidate()
            }
        }
    }

    /// Passes `request` to the engine at `base` (its password added here), and its response back
    /// in pieces as they come: an event stream's events one by one.
    public static func forward(_ request: LinkRequest, to base: URL, password: String?, session: URLSession = .shared,
                               allowing: @Sendable (String, String) -> Bool = { allows($0, method: $1) }) -> AsyncThrowingStream<LinkResponsePart, Error> {
        AsyncThrowingStream { continuation in
            guard allowing(request.path, request.method) else {
                continuation.yield(.head(status: 403, headers: ["content-type": "text/plain"]))
                continuation.yield(.body(Data("That isn’t available over the link.".utf8)))
                continuation.finish()
                return
            }
            let task = Task {
                do {
                    guard let url = URL(string: request.path, relativeTo: base) else { throw URLError(.badURL) }
                    var outgoing = URLRequest(url: url)
                    let method = request.method.uppercased()
                    outgoing.httpMethod = method
                    outgoing.timeoutInterval = .infinity
                    let given = Dictionary(request.headers.map { ($0.key.lowercased(), $0.value) }) { first, _ in first }
                    for name in requestHeaders { if let value = given[name] { outgoing.setValue(value, forHTTPHeaderField: name) } }
                    if let password { outgoing.setValue("Basic " + Data("opencode:\(password)".utf8).base64EncodedString(), forHTTPHeaderField: "Authorization") }
                    if method != "GET", method != "HEAD" { outgoing.httpBody = request.body }

                    let (bytes, response) = try await session.bytes(for: outgoing)
                    guard let http = response as? HTTPURLResponse else { throw URLError(.badServerResponse) }
                    var headers: [String: String] = [:]
                    for name in responseHeaders { if let value = http.value(forHTTPHeaderField: name) { headers[name] = value } }
                    continuation.yield(.head(status: http.statusCode, headers: headers))
                    // In pieces: at each event's end (a blank line), so an event goes as soon as
                    // it's whole and each costs the relay one message, or every 32 KB of a long body.
                    var piece = Data()
                    piece.reserveCapacity(32 * 1024)
                    var previous: UInt8 = 0
                    for try await byte in bytes {
                        piece.append(byte)
                        defer { previous = byte }
                        if (byte == UInt8(ascii: "\n") && previous == UInt8(ascii: "\n")) || piece.count >= 32 * 1024 {
                            continuation.yield(.body(piece))
                            piece.removeAll(keepingCapacity: true)
                        }
                    }
                    if !piece.isEmpty { continuation.yield(.body(piece)) }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

/// Says when a WebSocket task opens, or that it never will.
private final class SocketWatcher: NSObject, URLSessionWebSocketDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var outcome: Result<Void, Error>?
    private var waiting: CheckedContinuation<Void, Error>?

    func opened() async throws {
        try await withCheckedThrowingContinuation { continuation in
            lock.withLock {
                if let outcome { continuation.resume(with: outcome) } else { waiting = continuation }
            }
        }
    }

    private func settle(_ result: Result<Void, Error>) {
        lock.withLock {
            guard outcome == nil else { return }
            outcome = result
            waiting?.resume(with: result)
            waiting = nil
        }
    }

    func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask, didOpenWithProtocol protocol: String?) {
        settle(.success(()))
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        settle(.failure(error ?? URLError(.networkConnectionLost)))
    }
}
