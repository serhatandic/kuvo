import Foundation

struct EngineVersion: Decodable, Sendable {
    let Version: String
    let Platform: Platform?
    struct Platform: Decodable, Sendable { let Name: String }
}

struct Container: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let names: [String]
    let image: String
    let state: State
    let status: String
    let created: Date
    let ports: [Port]
    let labels: [String: String]

    struct Port: Decodable, Hashable, Sendable {
        let privatePort: Int
        let publicPort: Int?
        let type: String
        enum CodingKeys: String, CodingKey { case privatePort = "PrivatePort", publicPort = "PublicPort", type = "Type" }
    }

    enum State: String, Decodable, Sendable {
        case created, running, paused, restarting, removing, exited, dead
        init(from decoder: Decoder) throws {
            self = State(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .dead
        }
    }

    enum CodingKeys: String, CodingKey {
        case id = "Id", names = "Names", image = "Image", state = "State", status = "Status"
        case created = "Created", ports = "Ports", labels = "Labels"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        names = try c.decodeIfPresent([String].self, forKey: .names) ?? []
        image = try c.decode(String.self, forKey: .image)
        state = try c.decode(State.self, forKey: .state)
        status = try c.decode(String.self, forKey: .status)
        created = Date(timeIntervalSince1970: try c.decode(Double.self, forKey: .created))
        ports = try c.decodeIfPresent([Port].self, forKey: .ports) ?? []
        labels = try c.decodeIfPresent([String: String].self, forKey: .labels) ?? [:]
    }

    var name: String { names.first.map { String($0.drop { $0 == "/" }) } ?? String(id.prefix(12)) }
    var shortID: String { String(id.prefix(12)) }
    var project: String? { labels["com.docker.compose.project"] }
    var service: String? { labels["com.docker.compose.service"] }
    var isRunning: Bool { state == .running || state == .restarting }

    /// Unique published host ports, e.g. `[8080, 5432]`.
    var publishedPorts: [Int] {
        var seen = Set<Int>()
        return ports.compactMap(\.publicPort).filter { seen.insert($0).inserted }.sorted()
    }
}

/// The subset of `/containers/{id}/stats` we care about.
struct StatsSample: Decodable, Sendable {
    let cpu_stats: CPU
    let precpu_stats: CPU
    let memory_stats: Memory
    let networks: [String: Network]?

    struct CPU: Decodable, Sendable {
        let cpu_usage: Usage
        let system_cpu_usage: UInt64?
        let online_cpus: Int?
        struct Usage: Decodable, Sendable { let total_usage: UInt64 }
    }
    struct Memory: Decodable, Sendable {
        let usage: UInt64?
        let limit: UInt64?
        let stats: [String: UInt64]?
    }
    struct Network: Decodable, Sendable { let rx_bytes: UInt64; let tx_bytes: UInt64 }

    /// Percent of a single core, the way `docker stats` reports it.
    var cpuPercent: Double {
        let cpuDelta = Double(cpu_stats.cpu_usage.total_usage) - Double(precpu_stats.cpu_usage.total_usage)
        let systemDelta = Double(cpu_stats.system_cpu_usage ?? 0) - Double(precpu_stats.system_cpu_usage ?? 0)
        guard cpuDelta > 0, systemDelta > 0 else { return 0 }
        return cpuDelta / systemDelta * Double(cpu_stats.online_cpus ?? 1) * 100
    }

    /// Working set: usage minus reclaimable page cache.
    var memoryUsed: UInt64 {
        let usage = memory_stats.usage ?? 0
        let cache = memory_stats.stats?["inactive_file"] ?? memory_stats.stats?["total_inactive_file"] ?? 0
        return usage > cache ? usage - cache : usage
    }

    var memoryLimit: UInt64 { memory_stats.limit ?? 0 }
    var rxBytes: UInt64 { networks?.values.reduce(0) { $0 + $1.rx_bytes } ?? 0 }
    var txBytes: UInt64 { networks?.values.reduce(0) { $0 + $1.tx_bytes } ?? 0 }
}

/// Splits a byte stream into lines, buffering partial lines across chunks.
struct LineSplitter {
    private var pending = Data()

    mutating func feed(_ data: Data) -> [Data] {
        pending.append(data)
        var lines: [Data] = []
        while let newline = pending.firstIndex(of: 10) {
            lines.append(pending[pending.startIndex..<newline])
            pending = pending[(newline + 1)...]
        }
        if pending.isEmpty { pending = Data() }
        return lines
    }
}
