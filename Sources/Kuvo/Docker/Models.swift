import Foundation

struct EngineVersion: Decodable, Sendable {
    let Version: String
    let Platform: Platform?
    struct Platform: Decodable, Sendable { let Name: String }
}

struct Container: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let names: [String]
    /// The name it was run with, like nginx:alpine.
    let image: String
    /// The image it actually runs, which keeps working after its tag moves to a newer image.
    let imageID: String
    let state: State
    let status: String
    let created: Date
    let ports: [Port]
    let labels: [String: String]
    /// The volumes it mounts, and where.
    let volumeMounts: [VolumeMount]

    struct VolumeMount: Hashable, Sendable {
        let name: String
        /// The path inside the container, like /var/lib/postgresql/data.
        let destination: String
        let readOnly: Bool
    }

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
        case id = "Id", names = "Names", image = "Image", imageID = "ImageID", state = "State", status = "Status"
        case created = "Created", ports = "Ports", labels = "Labels", mounts = "Mounts"
    }

    private struct Mount: Decodable {
        let type: String?
        let name: String?
        let destination: String?
        let rw: Bool?
        enum CodingKeys: String, CodingKey { case type = "Type", name = "Name", destination = "Destination", rw = "RW" }
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        names = try c.decodeIfPresent([String].self, forKey: .names) ?? []
        image = try c.decode(String.self, forKey: .image)
        imageID = try c.decodeIfPresent(String.self, forKey: .imageID) ?? ""
        state = try c.decode(State.self, forKey: .state)
        status = try c.decode(String.self, forKey: .status)
        created = Date(timeIntervalSince1970: try c.decode(Double.self, forKey: .created))
        ports = try c.decodeIfPresent([Port].self, forKey: .ports) ?? []
        labels = try c.decodeIfPresent([String: String].self, forKey: .labels) ?? [:]
        volumeMounts = (try c.decodeIfPresent([Mount].self, forKey: .mounts) ?? []).compactMap { mount in
            guard mount.type == "volume", let name = mount.name else { return nil }
            return VolumeMount(name: name, destination: mount.destination ?? "", readOnly: mount.rw == false)
        }
    }

    /// Names of the volumes it mounts.
    var volumes: [String] { volumeMounts.map(\.name) }
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

/// An image from `/images/json`: one per ID, with every tag pointing at it.
struct DockerImage: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    /// Like nginx:alpine; empty for an image whose tag moved to a newer one.
    let tags: [String]
    /// Like nginx@sha256:…; kept after the tag moves, so an untagged image still has a name.
    let digests: [String]
    let created: Date
    /// Bytes on disk, unpacked.
    let size: Int64
    let labels: [String: String]

    var shortID: String { String(id.drop { $0 != ":" }.dropFirst().prefix(12)) }
    var isUntagged: Bool { tags.isEmpty }
    /// Like nginx or ghcr.io/owner/app.
    var repository: String? {
        if let tag = tags.first { return Self.repository(of: tag) }
        return digests.first?.split(separator: "@").first.map(String.init)
    }

    /// nginx:alpine → nginx, localhost:5000/app:dev → localhost:5000/app.
    static func repository(of reference: String) -> String {
        guard let colon = reference.lastIndex(of: ":"), !reference[colon...].contains("/") else { return reference }
        return String(reference[..<colon])
    }

    enum CodingKeys: String, CodingKey {
        case id = "Id", tags = "RepoTags", digests = "RepoDigests", created = "Created", size = "Size", labels = "Labels"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        // Older engines list an untagged image as "<none>:<none>".
        tags = (try c.decodeIfPresent([String].self, forKey: .tags) ?? []).filter { $0 != "<none>:<none>" }.sorted()
        digests = try c.decodeIfPresent([String].self, forKey: .digests) ?? []
        created = Date(timeIntervalSince1970: try c.decodeIfPresent(Double.self, forKey: .created) ?? 0)
        size = try c.decodeIfPresent(Int64.self, forKey: .size) ?? 0
        labels = try c.decodeIfPresent([String: String].self, forKey: .labels) ?? [:]
    }
}

/// The subset of `/images/{id}/json` the detail view shows.
struct ImageDetails: Decodable, Sendable {
    let Id: String
    let Architecture: String?
    let Os: String?
    let Variant: String?
    let Config: ImageConfig?

    struct ImageConfig: Decodable, Sendable {
        let Cmd: [String]?
        let Entrypoint: [String]?
        let ExposedPorts: [String: Empty]?
        let WorkingDir: String?
        struct Empty: Decodable, Sendable {}
    }

    /// Like linux/arm64 or linux/amd64.
    var platform: String? {
        guard let Os, let Architecture else { return nil }
        return [Os, Architecture, Variant].compactMap(\.self).filter { !$0.isEmpty }.joined(separator: "/")
    }

    /// What a container runs by default: entrypoint, then command.
    var command: String? {
        let parts = (Config?.Entrypoint ?? []) + (Config?.Cmd ?? [])
        return parts.isEmpty ? nil : parts.joined(separator: " ")
    }

    /// Like ["80/tcp", "443/tcp"].
    var exposedPorts: [String] { (Config?.ExposedPorts?.keys).map { $0.sorted() } ?? [] }
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
