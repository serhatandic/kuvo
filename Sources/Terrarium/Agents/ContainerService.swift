import Foundation

/// What Terrarium adds on top of Docker, for any container: commands that
/// leave a record, file copies, archiving, and quick containers to work in.
/// Shared by the app and the `terrarium` CLI.
struct ContainerService: Sendable {
    static let creatorLabel = "dev.terrarium.creator"
    static let defaultImage = "ubuntu:24.04"
    static let workspace = "/workspace"

    let client: DockerClient
    var creator = Self.detectCreator()

    /// Who's driving: Claude Code sets CLAUDECODE=1 in its shells.
    static func detectCreator() -> String {
        let env = ProcessInfo.processInfo.environment
        if let creator = env["TERRARIUM_CREATOR"], !creator.isEmpty { return creator }
        return env["CLAUDECODE"] == "1" ? "Claude Code" : "you"
    }

    // MARK: Lifecycle

    /// A long-lived container to work in: idles until given commands, and
    /// comes back after Terrarium restarts unless archived.
    func create(name: String?, image: String, network: Bool, cpus: Double?, memoryMB: Int?) async throws -> Container {
        try await client.pull(image)
        var host: [String: Any] = [
            "Init": true, "NetworkMode": network ? "bridge" : "none", "RestartPolicy": ["Name": "unless-stopped"],
        ]
        if let cpus { host["NanoCpus"] = Int(cpus * 1_000_000_000) }
        if let memoryMB { host["Memory"] = memoryMB * 1024 * 1024 }

        struct Created: Decodable { let Id: String }
        let path = name.map { "/containers/create?name=\($0.urlQuery)" } ?? "/containers/create"
        let created = try await client.send("POST", path, json: [
            "Image": image,
            "Entrypoint": [String](),
            "Cmd": ["sleep", "infinity"],
            "WorkingDir": Self.workspace,
            "Labels": [Self.creatorLabel: creator],
            "HostConfig": host,
        ], as: Created.self)
        _ = try await client.send("POST", "/containers/\(created.Id)/start")
        Journal.append(.init(kind: .create, title: "Created from \(image)", detail: network ? nil : "No network"), to: created.Id)
        return try await resolve(created.Id)
    }

    /// By name, Compose service name (when unambiguous), or ID prefix.
    func resolve(_ reference: String) async throws -> Container {
        let all = try await client.containers()
        if let exact = all.first(where: { $0.name == reference || $0.id == reference }) { return exact }
        let matches = all.filter { $0.service == reference || (reference.count >= 3 && $0.id.hasPrefix(reference)) }
        if matches.count == 1 { return matches[0] }
        if matches.count > 1 {
            throw DockerError(message: "\(reference) is ambiguous: \(matches.map(\.name).joined(separator: ", ")).")
        }
        throw DockerError(message: "No container named \(reference). Run terrarium ls to see them.")
    }

    /// Stops it and keeps it; it's deleted after the retention period.
    func archive(_ container: Container) async throws {
        if container.isRunning { _ = try await client.send("POST", "/containers/\(container.id)/stop?t=2") }
        Journal.setArchived(container.id, Date())
    }

    func remove(_ container: Container) async throws {
        _ = try await client.send("DELETE", "/containers/\(container.id)?force=true")
        Journal.remove(container.id)
    }

    /// Running and unarchived, so commands can execute.
    func wake(_ container: Container) async throws {
        Journal.setArchived(container.id, nil)
        if container.state == .paused { _ = try await client.send("POST", "/containers/\(container.id)/unpause") }
        if !container.isRunning && container.state != .paused { _ = try await client.send("POST", "/containers/\(container.id)/start") }
    }

    // MARK: Work

    /// Runs a command and records it in the container's activity.
    /// `started` lets a caller close the record if it's interrupted.
    func exec(_ container: Container, command: String, workdir: String?,
              onOutput: @Sendable (StreamDemuxer.Kind, Data) -> Void = { _, _ in },
              started: (Journal.Entry) -> Void = { _ in }) async throws -> ExecResult {
        try await wake(container)
        let entry = Journal.Entry(kind: .exec, title: command)
        Journal.append(entry, to: container.id)
        started(entry)
        let result = try await client.execute(in: container.id, command: command, workdir: workdir, onOutput: onOutput)
        var done = entry
        done.finish(exitCode: result.exitCode, duration: result.duration, output: result.output)
        Journal.append(done, to: container.id)
        return result
    }

    func readFile(_ container: Container, path: String) async throws -> Data {
        let path = try await absolute(path, in: container)
        let tar = try await client.send("GET", "/containers/\(container.id)/archive?path=\(path.urlQuery)")
        guard let data = Tar.firstFile(in: tar) else { throw DockerError(message: "\(path) isn't a regular file") }
        return data
    }

    /// Copies a folder (or file) from the Mac into the container. A copy, never a mount.
    func copyIn(_ container: Container, hostPath: String, destination: String?) async throws -> (path: String, files: Int, bytes: Int) {
        try await wake(container)
        let source = URL(filePath: (hostPath as NSString).expandingTildeInPath).standardizedFileURL
        let destination = try await absolute(destination ?? ".", in: container)
        let (entries, files, bytes) = try HostFiles.entries(for: source)
        try await makeDirectory(container, destination)
        _ = try await client.send("PUT", "/containers/\(container.id)/archive?path=\(destination.urlQuery)",
                                  body: Tar.archive(entries), contentType: "application/x-tar")
        let target = (destination as NSString).appendingPathComponent(source.lastPathComponent)
        let summary = "\(files) files, \(Format.plainBytes(Double(bytes)))"
        Journal.append(.init(kind: .copy, title: "\(source.path.abbreviatingHome) → \(target)", detail: summary, finished: true), to: container.id)
        return (target, files, bytes)
    }

    /// Relative paths are relative to the container's working directory.
    private func absolute(_ path: String, in container: Container) async throws -> String {
        guard !path.hasPrefix("/") else { return path }
        struct Inspect: Decodable { struct Config: Decodable { let WorkingDir: String? }; let Config: Config }
        let workdir = (try? await JSONDecoder().decode(Inspect.self, from: client.send("GET", "/containers/\(container.id)/json")))?
            .Config.WorkingDir.flatMap { $0.isEmpty ? nil : $0 } ?? "/"
        return ((workdir as NSString).appendingPathComponent(path) as NSString).standardizingPath
    }

    // MARK: Helpers

    private func makeDirectory(_ container: Container, _ path: String) async throws {
        guard path != "/" else { return }
        struct Created: Decodable { let Id: String }
        let exec = try await client.send("POST", "/containers/\(container.id)/exec", json: ["Cmd": ["mkdir", "-p", path]], as: Created.self)
        _ = try await client.send("POST", "/exec/\(exec.Id)/start", body: JSONSerialization.data(withJSONObject: ["Detach": false]))
    }

}

/// Files to copy from the Mac, honoring .gitignore when the folder is a git repo.
enum HostFiles {
    static let maxBytes = 1 << 30
    private static let skipped: Set<String> = [".git", "node_modules", ".build", "DerivedData", ".venv", "__pycache__", ".DS_Store"]

    static func entries(for source: URL) throws -> ([Tar.Entry], files: Int, bytes: Int) {
        let fm = FileManager.default
        var isDirectory: ObjCBool = false
        guard fm.fileExists(atPath: source.path, isDirectory: &isDirectory) else {
            throw DockerError(message: "\(source.path) doesn't exist on this Mac")
        }
        let root = source.lastPathComponent
        guard isDirectory.boolValue else {
            let data = try Data(contentsOf: source)
            return ([.file(path: root, data: data, mode: mode(source))], 1, data.count)
        }

        let paths = gitFiles(source) ?? walk(source)
        var entries: [Tar.Entry] = [.directory(path: root)]
        var directories: Set<String> = [root]
        var bytes = 0
        for relative in paths.sorted() {
            let url = source.appending(path: relative)
            // Parent directories first, so permissions and structure survive.
            var parent = (relative as NSString).deletingLastPathComponent
            var missing: [String] = []
            while !parent.isEmpty, !directories.contains("\(root)/\(parent)") {
                missing.append(parent)
                parent = (parent as NSString).deletingLastPathComponent
            }
            for dir in missing.reversed() {
                directories.insert("\(root)/\(dir)")
                entries.append(.directory(path: "\(root)/\(dir)"))
            }
            if let target = try? fm.destinationOfSymbolicLink(atPath: url.path) {
                entries.append(.symlink(path: "\(root)/\(relative)", target: target))
            } else if let data = try? Data(contentsOf: url) {
                bytes += data.count
                guard bytes <= maxBytes else { throw DockerError(message: "\(source.path) is over 1 GB. Copy a smaller folder.") }
                entries.append(.file(path: "\(root)/\(relative)", data: data, mode: mode(url)))
            }
        }
        return (entries, paths.count, bytes)
    }

    private static func mode(_ url: URL) -> Int {
        ((try? FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions]) as? Int) ?? 0o644
    }

    /// Tracked and untracked-but-not-ignored files. Only used when developer
    /// tools are installed, so we never trigger the "install git" prompt.
    private static func gitFiles(_ root: URL) -> [String]? {
        guard FileManager.default.fileExists(atPath: root.appending(path: ".git").path),
              run("/usr/bin/xcode-select", ["-p"]) != nil,
              let output = run("/usr/bin/git", ["-C", root.path, "ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        else { return nil }
        return output.split(separator: 0).map { String(decoding: $0, as: UTF8.self) }
            .filter { FileManager.default.fileExists(atPath: root.appending(path: $0).path) }
    }

    private static func walk(_ root: URL) -> [String] {
        guard let enumerator = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey]) else { return [] }
        var paths: [String] = []
        for case let url as URL in enumerator {
            if skipped.contains(url.lastPathComponent) { enumerator.skipDescendants(); continue }
            let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
            if values?.isDirectory == true, values?.isSymbolicLink != true { continue }
            paths.append(String(url.standardizedFileURL.path.dropFirst(root.standardizedFileURL.path.count + 1)))
        }
        return paths
    }

    private static func run(_ tool: String, _ arguments: [String]) -> Data? {
        let process = Process()
        process.executableURL = URL(filePath: tool)
        process.arguments = arguments
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        guard (try? process.run()) != nil else { return nil }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return process.terminationStatus == 0 ? data : nil
    }
}

// MARK: - Journal

/// What happened in each container, as JSON lines on disk. Written by the
/// CLI, read by the app. Also records when a container was archived.
enum Journal {
    struct Entry: Codable, Identifiable, Equatable, Sendable {
        enum Kind: String, Codable, Sendable { case create, exec, write, copy }

        var id = UUID()
        let kind: Kind
        var date = Date()
        let title: String
        var detail: String?
        var exitCode: Int?
        var duration: Double?
        var output: String?
        var finished = false

        mutating func finish(exitCode: Int?, duration: Double, output: String) {
            self.exitCode = exitCode
            self.duration = duration
            self.output = output
            finished = true
        }
    }

    static let directory = EnginePaths.support.appending(path: "activity", directoryHint: .isDirectory)

    static func append(_ entry: Entry, to id: String) {
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = file(id, "jsonl")
        guard var line = try? encoder.encode(entry) else { return }
        line.append(10)
        if let handle = try? FileHandle(forWritingTo: url) {
            _ = try? handle.seekToEnd()
            try? handle.write(contentsOf: line)
            try? handle.close()
        } else {
            try? line.write(to: url)
        }
    }

    /// Entries in order; a finished record replaces its start record.
    static func entries(_ id: String) -> [Entry] {
        guard let data = try? Data(contentsOf: file(id, "jsonl")) else { return [] }
        var order: [UUID] = []
        var latest: [UUID: Entry] = [:]
        for line in data.split(separator: 10) {
            guard let entry = try? decoder.decode(Entry.self, from: line) else { continue }
            if latest[entry.id] == nil { order.append(entry.id) }
            latest[entry.id] = entry
        }
        return order.compactMap { latest[$0] }
    }

    static func modified(_ id: String) -> Date? {
        (try? FileManager.default.attributesOfItem(atPath: file(id, "jsonl").path))?[.modificationDate] as? Date
    }

    static func archivedAt(_ id: String) -> Date? {
        (try? String(contentsOf: file(id, "archived"), encoding: .utf8)).flatMap { try? Date($0, strategy: .iso8601) }
    }

    static func setArchived(_ id: String, _ date: Date?) {
        if let date {
            try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try? date.formatted(.iso8601).write(to: file(id, "archived"), atomically: true, encoding: .utf8)
        } else {
            try? FileManager.default.removeItem(at: file(id, "archived"))
        }
    }

    static func remove(_ id: String) {
        for ext in ["jsonl", "archived"] { try? FileManager.default.removeItem(at: file(id, ext)) }
    }

    private static func file(_ id: String, _ ext: String) -> URL {
        directory.appending(path: "\(id).\(ext)")
    }

    private static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        return encoder
    }()

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}

extension Container {
    /// "Claude Code" or "you", for containers made with terrarium new.
    var creator: String? { labels[ContainerService.creatorLabel] }
}

extension String {
    var urlQuery: String { addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed.subtracting(["&", "=", "+", "?", "#"])) ?? self }
    var abbreviatingHome: String { (self as NSString).abbreviatingWithTildeInPath }
}
