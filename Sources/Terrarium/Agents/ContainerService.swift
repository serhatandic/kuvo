import Foundation

/// What Terrarium adds on top of Docker, for any container: commands,
/// file copies, archiving, and quick containers to work in.
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
        Archive.setArchived(container.id, Date())
    }

    func remove(_ container: Container) async throws {
        _ = try await client.send("DELETE", "/containers/\(container.id)?force=true")
        Archive.remove(container.id)
    }

    /// Running and unarchived, so commands can execute.
    func wake(_ container: Container) async throws {
        Archive.setArchived(container.id, nil)
        if container.state == .paused { _ = try await client.send("POST", "/containers/\(container.id)/unpause") }
        if !container.isRunning && container.state != .paused { _ = try await client.send("POST", "/containers/\(container.id)/start") }
    }

    // MARK: Work

    /// Runs a command, starting the container first if needed.
    func exec(_ container: Container, command: String, workdir: String?,
              onOutput: @Sendable (StreamDemuxer.Kind, Data) -> Void = { _, _ in }) async throws -> ExecResult {
        try await wake(container)
        return try await client.execute(in: container.id, command: command, workdir: workdir, onOutput: onOutput)
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

// MARK: - Archive

/// Which containers are archived, and since when: one small file per container,
/// shared by the app and the CLI.
enum Archive {
    static let directory = EnginePaths.support.appending(path: "archive", directoryHint: .isDirectory)

    static func archivedAt(_ id: String) -> Date? {
        (try? String(contentsOf: file(id), encoding: .utf8)).flatMap { try? Date($0, strategy: .iso8601) }
    }

    static func setArchived(_ id: String, _ date: Date?) {
        if let date {
            try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try? date.formatted(.iso8601).write(to: file(id), atomically: true, encoding: .utf8)
        } else {
            remove(id)
        }
    }

    static func remove(_ id: String) {
        try? FileManager.default.removeItem(at: file(id))
    }

    private static func file(_ id: String) -> URL { directory.appending(path: id) }
}

extension Container {
    /// "Claude Code" or "you", for containers made with terrarium new.
    var creator: String? { labels[ContainerService.creatorLabel] }
}

extension String {
    var urlQuery: String { addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed.subtracting(["&", "=", "+", "?", "#"])) ?? self }
    var abbreviatingHome: String { (self as NSString).abbreviatingWithTildeInPath }
}
