import Foundation

/// Disposable Linux environments for agents. A sandbox is a plain container
/// with a label, plus a journal of what happened inside it. Nothing expires
/// on its own; archived sandboxes are deleted after a retention period.
struct Sandboxes: Sendable {
    static let label = "dev.terrarium.sandbox"
    static let clientLabel = "dev.terrarium.client"
    static let defaultImage = "ubuntu:24.04"
    static let workspace = "/workspace"

    let client: DockerClient
    var clientName = "agent"

    // MARK: Lifecycle

    func list() async throws -> [Container] {
        let filter = #"{"label":["\#(Self.label)"]}"#.addingPercentEncoding(withAllowedCharacters: .alphanumerics)!
        return try await JSONDecoder().decode([Container].self, from: client.send("GET", "/containers/json?all=true&filters=\(filter)"))
    }

    func create(name: String?, image: String, network: Bool, cpus: Double?, memoryMB: Int?) async throws -> Container {
        try await client.pull(image)
        let name = name ?? Self.generatedName()
        // Back after Terrarium restarts, unless archived (stopped on purpose).
        var host: [String: Any] = [
            "Init": true, "NetworkMode": network ? "bridge" : "none", "RestartPolicy": ["Name": "unless-stopped"],
        ]
        if let cpus { host["NanoCpus"] = Int(cpus * 1_000_000_000) }
        if let memoryMB { host["Memory"] = memoryMB * 1024 * 1024 }

        struct Created: Decodable { let Id: String }
        let created = try await client.send("POST", "/containers/create?name=\(name.urlQuery)", json: [
            "Image": image,
            "Entrypoint": [String](),
            "Cmd": ["sleep", "infinity"],
            "WorkingDir": Self.workspace,
            "Labels": [Self.label: "1", Self.clientLabel: clientName],
            "HostConfig": host,
        ], as: Created.self)
        _ = try await client.send("POST", "/containers/\(created.Id)/start")
        Journal.append(.init(kind: .create, title: "Created from \(image)", detail: network ? nil : "No network"), to: created.Id)
        return try await resolve(created.Id)
    }

    /// Finds a sandbox by name or ID prefix.
    func resolve(_ reference: String) async throws -> Container {
        let all = try await list()
        if let match = all.first(where: { $0.name == reference || $0.id.hasPrefix(reference) }) { return match }
        let names = all.map(\.name).joined(separator: ", ")
        throw DockerError(message: "No sandbox named \(reference).\(names.isEmpty ? "" : " Existing: \(names)")")
    }

    func archive(_ sandbox: Container) async throws {
        if sandbox.isRunning { _ = try await client.send("POST", "/containers/\(sandbox.id)/stop?t=2") }
        Journal.setArchived(sandbox.id, Date())
    }

    func destroy(_ sandbox: Container) async throws {
        _ = try await client.send("DELETE", "/containers/\(sandbox.id)?force=true")
        Journal.remove(sandbox.id)
    }

    /// Running and unarchived, so commands can execute.
    func wake(_ sandbox: Container) async throws {
        Journal.setArchived(sandbox.id, nil)
        if sandbox.state == .paused { _ = try await client.send("POST", "/containers/\(sandbox.id)/unpause") }
        if !sandbox.isRunning && sandbox.state != .paused { _ = try await client.send("POST", "/containers/\(sandbox.id)/start") }
    }

    // MARK: Work

    func exec(_ sandbox: Container, command: String, workdir: String?, timeout: Duration) async throws -> ExecResult {
        try await wake(sandbox)
        let entry = Journal.Entry(kind: .exec, title: command)
        Journal.append(entry, to: sandbox.id)
        let result = try await client.execute(in: sandbox.id, command: command, workdir: workdir ?? Self.workspace, timeout: timeout)
        var done = entry
        done.finish(exitCode: result.exitCode, duration: result.duration, output: result.output)
        Journal.append(done, to: sandbox.id)
        return result
    }

    func writeFile(_ sandbox: Container, path: String, content: Data) async throws {
        try await wake(sandbox)
        let path = Self.absolute(path)
        let directory = (path as NSString).deletingLastPathComponent
        try await makeDirectory(sandbox, directory)
        let tar = Tar.archive([.file(path: (path as NSString).lastPathComponent, data: content)])
        _ = try await client.send("PUT", "/containers/\(sandbox.id)/archive?path=\(directory.urlQuery)", body: tar, contentType: "application/x-tar")
        Journal.append(.init(kind: .write, title: path, detail: Format.plainBytes(Double(content.count)), finished: true), to: sandbox.id)
    }

    func readFile(_ sandbox: Container, path: String) async throws -> Data {
        let path = Self.absolute(path)
        let tar = try await client.send("GET", "/containers/\(sandbox.id)/archive?path=\(path.urlQuery)")
        guard let data = Tar.firstFile(in: tar) else { throw DockerError(message: "\(path) isn't a regular file") }
        return data
    }

    /// Copies a folder (or file) from the Mac into the sandbox. A copy, never a mount.
    func copyIn(_ sandbox: Container, hostPath: String, destination: String?) async throws -> (path: String, files: Int, bytes: Int) {
        try await wake(sandbox)
        let source = URL(filePath: (hostPath as NSString).expandingTildeInPath).standardizedFileURL
        let destination = Self.absolute(destination ?? Self.workspace)
        let (entries, files, bytes) = try HostFiles.entries(for: source)
        try await makeDirectory(sandbox, destination)
        _ = try await client.send("PUT", "/containers/\(sandbox.id)/archive?path=\(destination.urlQuery)",
                                  body: Tar.archive(entries), contentType: "application/x-tar")
        let target = (destination as NSString).appendingPathComponent(source.lastPathComponent)
        let summary = "\(files) files, \(Format.plainBytes(Double(bytes)))"
        Journal.append(.init(kind: .copy, title: "\(source.path.abbreviatingHome) → \(target)", detail: summary, finished: true), to: sandbox.id)
        return (target, files, bytes)
    }

    // MARK: Helpers

    private func makeDirectory(_ sandbox: Container, _ path: String) async throws {
        guard path != "/" else { return }
        struct Created: Decodable { let Id: String }
        let exec = try await client.send("POST", "/containers/\(sandbox.id)/exec", json: ["Cmd": ["mkdir", "-p", path]], as: Created.self)
        _ = try await client.send("POST", "/exec/\(exec.Id)/start", body: JSONSerialization.data(withJSONObject: ["Detach": false]))
    }

    private static func absolute(_ path: String) -> String {
        path.hasPrefix("/") ? path : (workspace as NSString).appendingPathComponent(path)
    }

    private static func generatedName() -> String {
        let letters = Array("abcdefghjkmnpqrstuvwxyz23456789")
        return "sandbox-" + String((0..<4).map { _ in letters.randomElement()! })
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

/// What happened in each sandbox, as JSON lines on disk. Written by the MCP
/// server, read by the app. Also records when a sandbox was archived.
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

    static let directory = EnginePaths.support.appending(path: "sandboxes", directoryHint: .isDirectory)

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
    var isSandbox: Bool { labels[Sandboxes.label] != nil }
    var sandboxClient: String? { labels[Sandboxes.clientLabel] }
}

extension String {
    var urlQuery: String { addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed.subtracting(["&", "=", "+", "?", "#"])) ?? self }
    var abbreviatingHome: String { (self as NSString).abbreviatingWithTildeInPath }
}
