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
    /// Names of the volumes it mounts.
    let volumes: [String]

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
        case created = "Created", ports = "Ports", labels = "Labels", mounts = "Mounts"
    }

    private struct Mount: Decodable {
        let type: String?
        let name: String?
        enum CodingKeys: String, CodingKey { case type = "Type", name = "Name" }
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
        volumes = (try c.decodeIfPresent([Mount].self, forKey: .mounts) ?? []).compactMap { $0.type == "volume" ? $0.name : nil }
    }

    var name: String { names.first.map { String($0.drop { $0 == "/" }) } ?? String(id.prefix(12)) }
    var shortID: String { String(id.prefix(12)) }
    var project: String? { labels["com.docker.compose.project"] }
    var service: String? { labels["com.docker.compose.service"] }
    var isRunning: Bool { state == .running || state == .restarting }

    /// Docker's status line in plain English: "Up 25 minutes (healthy)"
    /// becomes "Running for 25 minutes, healthy".
    var humanStatus: String {
        let health = ["unhealthy", "healthy", "health: starting"].first { status.contains("(\($0))") }
        let detail = status.replacing(/\s*\(.*?\)/, with: "")
        func lower(_ s: Substring) -> String { s.prefix(1).lowercased() + s.dropFirst() }

        switch state {
        case .running:
            let uptime = detail.hasPrefix("Up ") ? "Running for \(lower(detail.dropFirst(3)))" : "Running"
            return health.map { "\(uptime), \($0.replacingOccurrences(of: "health: ", with: ""))" } ?? uptime
        case .exited:
            let ago = detail.hasPrefix("Exited ") ? lower(detail.dropFirst(7)) : ""
            let code = status.firstMatch(of: /Exited \((\d+)\)/).flatMap { Int($0.1) } ?? 0
            return code == 0 ? "Stopped \(ago)" : "Exited with code \(code), \(ago)"
        case .created: return "Created, never started"
        default: return state.label
        }
    }

    /// Unique published host ports, e.g. `[8080, 5432]`.
    var publishedPorts: [Int] {
        var seen = Set<Int>()
        return ports.compactMap(\.publicPort).filter { seen.insert($0).inserted }.sorted()
    }
}

/// The subset of `/containers/{id}/json` we care about.
struct ContainerDetails: Decodable, Sendable {
    let State: StateInfo

    struct StateInfo: Decodable, Sendable {
        let StartedAt: String
        let FinishedAt: String
        let ExitCode: Int
        let Health: HealthInfo?
    }
    struct HealthInfo: Decodable, Sendable { let Status: String }

    var startedAt: Date? { dockerDate(State.StartedAt) }
    var finishedAt: Date? { dockerDate(State.FinishedAt) }
}

/// Docker uses nanosecond RFC 3339 timestamps and "0001-01-01..." for never.
private func dockerDate(_ string: String) -> Date? {
    guard !string.hasPrefix("0001") else { return nil }
    let trimmed = string.replacing(/\.\d+/, with: "")
    return try? Date(trimmed, strategy: .iso8601)
}

/// A volume as `/system/df` reports it: `/volumes` leaves out the size.
struct Volume: Decodable, Identifiable, Hashable, Sendable {
    let name: String
    let driver: String
    let mountpoint: String
    let createdAt: Date?
    let labels: [String: String]
    /// Bytes on disk; nil when Docker can't measure it (volumes from other drivers).
    let size: Int64?

    var id: String { name }
    var project: String? { labels["com.docker.compose.project"] }
    /// Made by `-v /path` or an image's VOLUME, named with a random 64-character ID.
    var isAnonymous: Bool {
        labels["com.docker.volume.anonymous"] != nil || (name.count == 64 && name.allSatisfy(\.isHexDigit))
    }
    var displayName: String { isAnonymous ? String(name.prefix(12)) : name }

    enum CodingKeys: String, CodingKey {
        case name = "Name", driver = "Driver", mountpoint = "Mountpoint", createdAt = "CreatedAt"
        case labels = "Labels", usage = "UsageData"
    }

    private struct Usage: Decodable {
        let size: Int64
        enum CodingKeys: String, CodingKey { case size = "Size" }
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        driver = try c.decodeIfPresent(String.self, forKey: .driver) ?? "local"
        mountpoint = try c.decodeIfPresent(String.self, forKey: .mountpoint) ?? ""
        createdAt = try c.decodeIfPresent(String.self, forKey: .createdAt).flatMap(dockerDate)
        labels = try c.decodeIfPresent([String: String].self, forKey: .labels) ?? [:]
        // -1 means not measured.
        size = try c.decodeIfPresent(Usage.self, forKey: .usage).flatMap { $0.size >= 0 ? $0.size : nil }
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
