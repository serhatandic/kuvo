import Foundation
import Network

struct DockerError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// Talks to the Docker Engine API over its Unix socket. No dependencies,
/// no `docker` CLI: just HTTP/1.1 over Network.framework.
struct DockerClient: Sendable {
    let socketPath: String

    /// An engine that's already running elsewhere: Docker Desktop, OrbStack,
    /// Colima, Rancher Desktop, or `DOCKER_HOST`.
    static func discoverExternal() -> DockerClient? {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        var candidates = [
            "\(home)/.docker/run/docker.sock",
            "\(home)/.orbstack/run/docker.sock",
            "\(home)/.colima/default/docker.sock",
            "\(home)/.rd/docker.sock",
            "/var/run/docker.sock",
        ]
        if let host = ProcessInfo.processInfo.environment["DOCKER_HOST"], host.hasPrefix("unix://") {
            candidates.insert(String(host.dropFirst(7)), at: 0)
        }
        return candidates.first { FileManager.default.fileExists(atPath: $0) }.map(DockerClient.init)
    }

    // MARK: Endpoints

    func ping() async throws { _ = try await send("GET", "/_ping") }

    func version() async throws -> EngineVersion {
        try await JSONDecoder().decode(EngineVersion.self, from: send("GET", "/version"))
    }

    func containers() async throws -> [Container] {
        try await JSONDecoder().decode([Container].self, from: send("GET", "/containers/json?all=true"))
    }

    func inspect(_ id: String) async throws -> ContainerDetails {
        try await JSONDecoder().decode(ContainerDetails.self, from: send("GET", "/containers/\(id)/json"))
    }

    func perform(_ action: ContainerAction, on id: String) async throws {
        switch action {
        case .start:   _ = try await send("POST", "/containers/\(id)/start")
        case .stop:    _ = try await send("POST", "/containers/\(id)/stop?t=5")
        case .restart: _ = try await send("POST", "/containers/\(id)/restart?t=5")
        case .pause:   _ = try await send("POST", "/containers/\(id)/pause")
        case .unpause: _ = try await send("POST", "/containers/\(id)/unpause")
        case .remove:  _ = try await send("DELETE", "/containers/\(id)?force=true")
        }
    }

    /// Newline-delimited JSON, one `StatsSample` roughly every second.
    func stats(_ id: String) -> AsyncThrowingStream<Data, Error> {
        stream("GET", "/containers/\(id)/stats?stream=true")
    }

    func events() -> AsyncThrowingStream<Data, Error> {
        let filter = #"{"type":["container"]}"#.addingPercentEncoding(withAllowedCharacters: .alphanumerics)!
        return stream("GET", "/events?filters=\(filter)")
    }

    // MARK: Transport

    /// One-shot request; returns the full body, throws on non-2xx.
    func send(_ method: String, _ path: String, body: Data? = nil, contentType: String = "application/json") async throws -> Data {
        var response = Data()
        for try await piece in stream(method, path, body: body, contentType: contentType) { response.append(piece) }
        return response
    }

    /// Sends a JSON body and decodes a JSON response.
    func send<T: Decodable>(_ method: String, _ path: String, json: [String: Any], as _: T.Type) async throws -> T {
        let body = try JSONSerialization.data(withJSONObject: json)
        return try await JSONDecoder().decode(T.self, from: send(method, path, body: body))
    }

    /// Streams the decoded response body. Cancelling the consuming task closes the connection.
    func stream(_ method: String, _ path: String, body: Data? = nil, contentType: String = "application/json") -> AsyncThrowingStream<Data, Error> {
        AsyncThrowingStream { continuation in
            var head = "\(method) \(path) HTTP/1.1\r\nHost: docker\r\nUser-Agent: Terrarium\r\nContent-Length: \(body?.count ?? 0)\r\n"
            if body != nil { head += "Content-Type: \(contentType)\r\n" }
            let request = Data((head + "\r\n").utf8) + (body ?? Data())
            let exchange = Exchange(socketPath: socketPath, request: request, continuation: continuation)
            continuation.onTermination = { _ in exchange.cancel() }
            exchange.start()
        }
    }
}

/// One request/response over a fresh connection. All mutable state is
/// confined to the connection's serial queue.
private final class Exchange: @unchecked Sendable {
    private let connection: NWConnection
    private let request: Data
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation
    private let queue = DispatchQueue(label: "terrarium.socket")
    private var parser = HTTPParser()
    private var errorBody = Data()

    init(socketPath: String, request: Data, continuation: AsyncThrowingStream<Data, Error>.Continuation) {
        self.connection = NWConnection(to: .unix(path: socketPath), using: .tcp)
        self.request = request
        self.continuation = continuation
    }

    func start() {
        connection.stateUpdateHandler = { [self] state in
            switch state {
            case .ready:
                connection.send(content: request, completion: .contentProcessed { _ in })
                receive()
            case .waiting(let error), .failed(let error):
                finish(error)
            default:
                break
            }
        }
        connection.start(queue: queue)
    }

    func cancel() { connection.cancel() }

    private func receive() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 1 << 16) { [self] data, _, eof, error in
            if let data, !data.isEmpty {
                for piece in parser.feed(data) {
                    if isFailure { errorBody.append(piece) } else { continuation.yield(piece) }
                }
            }
            if let error { return finish(error) }
            guard eof || parser.isComplete else { return receive() }
            if !parser.hasHead {
                finish(DockerError(message: "Docker closed the connection"))
            } else if isFailure {
                let message = (try? JSONDecoder().decode([String: String].self, from: errorBody))?["message"]
                finish(DockerError(message: message ?? "Docker returned HTTP \(parser.status ?? 0)"))
            } else {
                finish()
            }
        }
    }

    private var isFailure: Bool { (parser.status ?? 0) >= 300 }

    private func finish(_ error: Error? = nil) {
        continuation.finish(throwing: error)
        connection.cancel()
    }
}

enum ContainerAction: String, Sendable {
    case start, stop, restart, pause, unpause, remove
}
