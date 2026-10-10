import Foundation

struct LogLine: Sendable {
    let date: Date?
    let text: String
    let isError: Bool
    /// Which source this came from when several are merged (a Compose project).
    var source: Int?
}

/// Splits Docker's multiplexed stdout/stderr stream. Non-TTY containers and
/// execs prefix each frame with an 8-byte header; TTY ones send raw bytes.
struct StreamDemuxer {
    enum Kind { case stdout, stderr }
    private var buffer = Data()
    private var multiplexed: Bool?

    mutating func feed(_ data: Data) -> [(Kind, Data)] {
        buffer.append(data)
        if multiplexed == nil {
            guard buffer.count >= 8 else { return [] }
            let b = [UInt8](buffer.prefix(4))
            multiplexed = b[0] <= 2 && b[1] == 0 && b[2] == 0 && b[3] == 0
        }
        guard multiplexed == true else {
            defer { buffer = Data() }
            return [(.stdout, buffer)]
        }
        var frames: [(Kind, Data)] = []
        while buffer.count >= 8 {
            let header = [UInt8](buffer.prefix(8))
            let size = Int(header[4]) << 24 | Int(header[5]) << 16 | Int(header[6]) << 8 | Int(header[7])
            guard buffer.count >= 8 + size else { break }
            frames.append((header[0] == 2 ? .stderr : .stdout, Data(buffer.dropFirst(8).prefix(size))))
            buffer = Data(buffer.dropFirst(8 + size))
        }
        return frames
    }
}

/// Decodes a container's log stream into timestamped lines.
struct LogDecoder {
    private var demuxer = StreamDemuxer()
    private var stdout = LineSplitter()
    private var stderr = LineSplitter()

    mutating func feed(_ data: Data) -> [LogLine] {
        demuxer.feed(data).flatMap { kind, payload in
            kind == .stderr
                ? stderr.feed(payload).map { Self.parse($0, isError: true) }
                : stdout.feed(payload).map { Self.parse($0, isError: false) }
        }
    }

    nonisolated(unsafe) private static let iso: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    /// Lines look like `2026-10-10T18:27:01.123456789Z message`.
    private static func parse(_ raw: Data, isError: Bool) -> LogLine {
        var text = String(decoding: raw, as: UTF8.self)
        if text.hasSuffix("\r") { text.removeLast() }
        guard let space = text.firstIndex(of: " "), text.first?.isNumber == true else {
            return LogLine(date: nil, text: text, isError: isError)
        }
        // Trim nanoseconds to milliseconds so ISO8601DateFormatter accepts it.
        var stamp = String(text[..<space])
        if let dot = stamp.firstIndex(of: "."), let z = stamp.lastIndex(of: "Z") {
            let fraction = stamp[stamp.index(after: dot)..<z].prefix(3)
            stamp = String(stamp[..<dot]) + "." + fraction + "Z"
        }
        return LogLine(date: iso.date(from: stamp), text: String(text[text.index(after: space)...]), isError: isError)
    }
}
