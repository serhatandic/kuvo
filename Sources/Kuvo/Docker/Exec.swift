import Foundation

struct ExecResult: Sendable {
    var exitCode: Int?
    var output: String
    var duration: Double
}

extension DockerClient {
    /// Runs a shell command in a running container, streaming its output as
    /// it arrives and keeping a trimmed copy for the record.
    func execute(in id: String, command: String, workdir: String? = nil,
                 onOutput: @Sendable (StreamDemuxer.Kind, Data) -> Void = { _, _ in }) async throws -> ExecResult {
        let start = ContinuousClock.now
        struct Created: Decodable { let Id: String }
        var config: [String: Any] = [
            "AttachStdout": true, "AttachStderr": true, "Tty": false,
            // Prefer a bash login shell (PATH, profiles) and fall back to sh.
            "Cmd": ["sh", "-c", #"if command -v bash >/dev/null 2>&1; then exec bash -lc "$1"; else exec sh -c "$1"; fi"#, "sh", command],
        ]
        if let workdir { config["WorkingDir"] = workdir }
        let exec = try await send("POST", "/containers/\(id)/exec", json: config, as: Created.self)

        let output = OutputBuffer()
        var demuxer = StreamDemuxer()
        let body = try JSONSerialization.data(withJSONObject: ["Detach": false, "Tty": false])
        for try await chunk in stream("POST", "/exec/\(exec.Id)/start", body: body) {
            for (kind, data) in demuxer.feed(chunk) {
                output.append(data)
                onOutput(kind, data)
            }
        }

        struct Inspect: Decodable { let ExitCode: Int? }
        let state = try? await JSONDecoder().decode(Inspect.self, from: send("GET", "/exec/\(exec.Id)/json"))
        let elapsed = start.duration(to: .now)
        let duration = Double(elapsed.components.seconds) + Double(elapsed.components.attoseconds) / 1e18
        return ExecResult(exitCode: state?.ExitCode, output: output.text, duration: duration)
    }

    /// Pulls an image unless it's already present.
    func pull(_ image: String) async throws {
        if (try? await send("GET", "/images/\(image.urlQuery)/json")) != nil { return }
        try await pullLatest(image)
    }

    /// Fetches the image's current version from its registry, reporting progress as
    /// layers download. Without a tag, Docker pulls `latest`.
    @discardableResult
    func pullLatest(_ image: String, progress: @Sendable (PullProgress) -> Void = { _ in }) async throws -> PullOutcome {
        var query = "fromImage=\(image.urlQuery)"
        if !image.contains("@"), let colon = image.lastIndex(of: ":"), !image[colon...].contains("/") {
            query = "fromImage=\(String(image[..<colon]).urlQuery)&tag=\(String(image[image.index(after: colon)...]).urlQuery)"
        } else if !image.contains("@") {
            query += "&tag=latest"
        }
        var lines = LineSplitter()
        var state = PullProgress()
        var outcome = PullOutcome.downloaded
        for try await chunk in stream("POST", "/images/create?\(query)") {
            for line in lines.feed(chunk) {
                guard let json = try? JSONSerialization.jsonObject(with: line) as? [String: Any] else { continue }
                if let message = json["error"] as? String {
                    throw DockerError(message: "Couldn't pull \(image): \(message)")
                }
                if let status = json["status"] as? String, status.hasPrefix("Status: Image is up to date") {
                    outcome = .upToDate
                }
                if state.update(json) { progress(state) }
            }
        }
        return outcome
    }

}

enum PullOutcome: Sendable {
    case upToDate, downloaded
}

/// Bytes downloaded across an image's layers. Docker reports each layer's size only
/// once it starts downloading it, so the total grows early in a pull.
struct PullProgress: Sendable, Equatable {
    private var current: [String: Int64] = [:]
    private var total: [String: Int64] = [:]
    private var finished: Set<String> = []

    var downloaded: Int64 { total.keys.reduce(0) { $0 + (finished.contains($1) ? total[$1]! : min(current[$1] ?? 0, total[$1]!)) } }
    var size: Int64 { total.values.reduce(0, +) }
    /// Nil until any layer's size is known.
    var fraction: Double? { size > 0 ? Double(downloaded) / Double(size) : nil }

    /// Returns whether anything changed.
    mutating func update(_ json: [String: Any]) -> Bool {
        guard let id = json["id"] as? String, let status = json["status"] as? String else { return false }
        let detail = json["progressDetail"] as? [String: Any]
        switch status {
        case "Downloading":
            guard let now = (detail?["current"] as? NSNumber)?.int64Value else { return false }
            if let size = (detail?["total"] as? NSNumber)?.int64Value, size > 0 { total[id] = size }
            current[id] = now
        case "Download complete", "Pull complete", "Already exists":
            guard finished.insert(id).inserted else { return false }
        default:
            return false
        }
        return true
    }
}

/// Collects exec output from a background task, keeping it to a readable size.
private final class OutputBuffer: @unchecked Sendable {
    private let lock = NSLock()
    private var data = Data()
    private static let limit = 64 * 1024

    func append(_ chunk: Data) {
        lock.withLock {
            data.append(chunk)
            // Keep the start (context) and the end (results), drop the middle.
            if data.count > Self.limit * 2 {
                data = data.prefix(Self.limit / 4) + Data("\n[…]\n".utf8) + data.suffix(Self.limit)
            }
        }
    }

    var text: String {
        lock.withLock { String(decoding: data, as: UTF8.self) }
    }
}

