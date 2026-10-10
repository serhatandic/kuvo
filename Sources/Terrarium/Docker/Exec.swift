import Foundation

struct ExecResult: Sendable {
    var exitCode: Int?
    var output: String
    var duration: Double
}

extension DockerClient {
    /// Runs a shell command in a running container and collects its output.
    /// Waits up to `timeout`; a longer command keeps running in the background.
    func execute(in id: String, command: String, workdir: String? = nil, timeout: Duration) async throws -> ExecResult {
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
        let body = try JSONSerialization.data(withJSONObject: ["Detach": false, "Tty": false])
        let finished = await withTaskGroup(of: Bool.self) { group in
            group.addTask {
                var demuxer = StreamDemuxer()
                do {
                    for try await chunk in stream("POST", "/exec/\(exec.Id)/start", body: body) {
                        for (_, data) in demuxer.feed(chunk) { output.append(data) }
                    }
                } catch {}
                return true
            }
            group.addTask {
                try? await Task.sleep(for: timeout)
                return false
            }
            let first = await group.next() ?? false
            group.cancelAll()
            return first
        }

        struct Inspect: Decodable { let ExitCode: Int?; let Running: Bool }
        let state = try? await JSONDecoder().decode(Inspect.self, from: send("GET", "/exec/\(exec.Id)/json"))
        var text = output.text
        if !finished { text += "\n[Still running after \(timeout). It continues in the background.]" }
        let elapsed = start.duration(to: .now)
        let duration = Double(elapsed.components.seconds) + Double(elapsed.components.attoseconds) / 1e18
        return ExecResult(exitCode: state?.Running == false ? state?.ExitCode : nil, output: text, duration: duration)
    }

    /// Pulls an image unless it's already present.
    func pull(_ image: String) async throws {
        if (try? await send("GET", "/images/\(image.urlQuery)/json")) != nil { return }
        var query = "fromImage=\(image.urlQuery)"
        if !image.contains("@"), let colon = image.lastIndex(of: ":"), !image[colon...].contains("/") {
            query = "fromImage=\(String(image[..<colon]).urlQuery)&tag=\(String(image[image.index(after: colon)...]).urlQuery)"
        }
        var lines = LineSplitter()
        for try await chunk in stream("POST", "/images/create?\(query)") {
            for line in lines.feed(chunk) {
                if let message = (try? JSONSerialization.jsonObject(with: line) as? [String: Any])?["error"] as? String {
                    throw DockerError(message: "Couldn't pull \(image): \(message)")
                }
            }
        }
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

