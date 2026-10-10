import Foundation

/// Just enough tar to move files in and out of containers, which Docker's
/// archive API speaks. Writes ustar, with PAX headers for long paths.
enum Tar {
    enum Entry {
        case file(path: String, data: Data, mode: Int = 0o644)
        case directory(path: String)
        case symlink(path: String, target: String)
    }

    static func archive(_ entries: [Entry]) -> Data {
        var out = Data()
        for entry in entries {
            switch entry {
            case .file(let path, let data, let mode):
                header(&out, path: path, mode: mode, size: data.count, type: "0")
                out.append(data)
                pad(&out)
            case .directory(let path):
                header(&out, path: path.hasSuffix("/") ? path : path + "/", mode: 0o755, size: 0, type: "5")
            case .symlink(let path, let target):
                header(&out, path: path, mode: 0o777, size: 0, type: "2", link: target)
            }
        }
        out.append(Data(count: 1024)) // Two empty blocks end the archive.
        return out
    }

    /// The first regular file in an archive.
    static func firstFile(in data: Data) -> Data? {
        var offset = 0
        while offset + 512 <= data.count {
            let block = data.subdata(in: offset..<offset + 512)
            guard block.contains(where: { $0 != 0 }) else { return nil }
            let size = Int(field(block, 124, 12).trimmingCharacters(in: .whitespaces), radix: 8) ?? 0
            let type = block[block.startIndex + 156]
            let start = offset + 512
            if type == UInt8(ascii: "0") || type == 0 {
                return data.subdata(in: start..<min(start + size, data.count))
            }
            offset = start + (size + 511) / 512 * 512
        }
        return nil
    }

    // MARK: Private

    private static func header(_ out: inout Data, path: String, mode: Int, size: Int, type: Character, link: String = "") {
        if path.utf8.count > 99 || link.utf8.count > 99 {
            // PAX extended header carries the long names.
            var records = ""
            for (key, value) in [("path", path), ("linkpath", link)] where !value.isEmpty {
                records += record(key, value)
            }
            let data = Data(records.utf8)
            header(&out, path: "PaxHeader", mode: 0o644, size: data.count, type: "x")
            out.append(data)
            pad(&out)
        }
        var block = Data(count: 512)
        put(&block, 0, 100, String(decoding: path.utf8.prefix(99), as: UTF8.self))
        put(&block, 100, 8, octal(mode, 7))
        put(&block, 108, 8, octal(0, 7))
        put(&block, 116, 8, octal(0, 7))
        put(&block, 124, 12, octal(size, 11))
        put(&block, 136, 12, octal(Int(Date().timeIntervalSince1970), 11))
        put(&block, 148, 8, "        ")
        put(&block, 156, 1, String(type))
        put(&block, 157, 100, String(decoding: link.utf8.prefix(99), as: UTF8.self))
        put(&block, 257, 8, "ustar\u{0}00")
        let checksum = block.reduce(0) { $0 + Int($1) }
        put(&block, 148, 8, octal(checksum, 6) + "\u{0} ")
        out.append(block)
    }

    /// A PAX record is "<length> <key>=<value>\n", where length counts itself.
    private static func record(_ key: String, _ value: String) -> String {
        let body = " \(key)=\(value)\n"
        var length = body.utf8.count + 1
        while "\(length)".count + body.utf8.count != length { length += 1 }
        return "\(length)\(body)"
    }

    private static func octal(_ value: Int, _ width: Int) -> String {
        let digits = String(value, radix: 8)
        return String(repeating: "0", count: max(width - digits.count, 0)) + digits
    }

    private static func put(_ block: inout Data, _ offset: Int, _ length: Int, _ string: String) {
        let bytes = Array(string.utf8.prefix(length))
        block.replaceSubrange(offset..<offset + bytes.count, with: bytes)
    }

    private static func field(_ block: Data, _ offset: Int, _ length: Int) -> String {
        let bytes = block[(block.startIndex + offset)..<(block.startIndex + offset + length)].prefix { $0 != 0 }
        return String(decoding: bytes, as: UTF8.self)
    }

    private static func pad(_ out: inout Data) {
        out.append(Data(count: (512 - out.count % 512) % 512))
    }
}
