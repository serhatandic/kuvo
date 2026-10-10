import Foundation

/// Incremental HTTP/1.1 response parser. Feed it raw bytes as they arrive;
/// it hands back decoded body pieces (handling chunked transfer encoding).
struct HTTPParser {
    private(set) var status: Int?
    private(set) var headers: [String: String] = [:]
    private(set) var isComplete = false

    private enum Framing { case chunked, length(Int), untilClose }
    private enum ChunkState { case size, data(Int), trailer }

    private var buffer: [UInt8] = []
    private var framing: Framing = .untilClose
    private var chunk: ChunkState = .size

    var hasHead: Bool { status != nil }

    mutating func feed(_ data: Data) -> [Data] {
        buffer.append(contentsOf: data)
        if status == nil, !parseHead() { return [] }
        return drainBody()
    }

    private mutating func parseHead() -> Bool {
        guard let end = buffer.firstRange(of: [13, 10, 13, 10]) else { return false }
        let head = String(decoding: buffer[..<end.lowerBound], as: UTF8.self)
        buffer.removeSubrange(..<end.upperBound)

        var lines = head.components(separatedBy: "\r\n")
        let statusLine = lines.removeFirst().split(separator: " ")
        status = statusLine.count > 1 ? Int(statusLine[1]) : 0
        for line in lines {
            guard let colon = line.firstIndex(of: ":") else { continue }
            let key = line[..<colon].lowercased()
            headers[key] = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        }

        if headers["transfer-encoding"]?.lowercased() == "chunked" {
            framing = .chunked
        } else if let length = headers["content-length"].flatMap(Int.init) {
            framing = .length(length)
            if length == 0 { isComplete = true }
        } else if status == 204 || status == 304 {
            isComplete = true
        }
        return true
    }

    private mutating func drainBody() -> [Data] {
        switch framing {
        case .untilClose:
            defer { buffer.removeAll(keepingCapacity: true) }
            return buffer.isEmpty ? [] : [Data(buffer)]

        case .length(let remaining):
            let n = min(remaining, buffer.count)
            guard n > 0 else { return [] }
            let piece = Data(buffer[..<n])
            buffer.removeSubrange(..<n)
            framing = .length(remaining - n)
            if remaining - n == 0 { isComplete = true }
            return [piece]

        case .chunked:
            var out: [Data] = []
            while !isComplete {
                switch chunk {
                case .size:
                    guard let crlf = buffer.firstRange(of: [13, 10]) else { return out }
                    let line = String(decoding: buffer[..<crlf.lowerBound], as: UTF8.self)
                    buffer.removeSubrange(..<crlf.upperBound)
                    let hex = line.split(separator: ";").first.map(String.init) ?? ""
                    let size = Int(hex.trimmingCharacters(in: .whitespaces), radix: 16) ?? 0
                    if size == 0 { isComplete = true } else { chunk = .data(size) }

                case .data(let remaining):
                    let n = min(remaining, buffer.count)
                    guard n > 0 else { return out }
                    out.append(Data(buffer[..<n]))
                    buffer.removeSubrange(..<n)
                    chunk = remaining - n == 0 ? .trailer : .data(remaining - n)

                case .trailer:
                    guard buffer.count >= 2 else { return out }
                    buffer.removeSubrange(..<2)
                    chunk = .size
                }
            }
            return out
        }
    }
}
