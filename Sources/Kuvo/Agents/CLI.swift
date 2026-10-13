import AppKit
import ArgumentParser
import Foundation

/// `kuvo`: work inside containers from the command line, written to
/// read well for people and for agents (plain output, real exit codes).
struct CLI: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "kuvo",
        abstract: "Work inside your containers: run commands, open shells, copy files.",
        discussion: """
        Works on any container, by name, Compose service name or ID prefix.

        Examples:
          kuvo new bench --cpus 2 --memory 2g
          kuvo exec bench "apt-get update && apt-get install -y hyperfine"
          kuvo cp ~/code/parser bench
          kuvo exec -w /workspace/parser bench hyperfine 'python3 bench.py'
          kuvo exec db pg_isready
        """,
        version: CLIInstaller.appBundle?.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev",
        subcommands: [New.self, List.self, Exec.self, ShellCommand.self, Copy.self,
                      ArchiveCommand.self, Restore.self, Remove.self, Status.self, Env.self]
    )

    struct New: AsyncParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Create a Linux container to work in.",
            discussion: "It idles in /workspace until given commands and stays until you archive or remove it."
        )

        @Argument(help: "Name, e.g. bench. Docker picks one if omitted.") var name: String?
        @Option(help: "Image.") var image = ContainerService.defaultImage
        @Flag(help: "Turn off network access.") var noNetwork = false
        @Option(help: "CPU limit, e.g. 2.") var cpus: Double?
        @Option(help: "Memory limit, e.g. 2g or 512m.") var memory: String?

        func run() async throws {
            let container = try await CLI.connect().create(
                name: name, image: image, network: !noNetwork, cpus: cpus, memoryMB: try memory.map(CLI.megabytes)
            )
            print("Created \(container.name) from \(image).")
            print("Run commands with: kuvo exec \(container.name) <command>")
        }
    }

    struct List: AsyncParsableCommand {
        static let configuration = CommandConfiguration(commandName: "ls", abstract: "List containers.")

        func run() async throws {
            let all = try await CLI.connect().client.containers()
            guard !all.isEmpty else { return print("No containers. Create one with: kuvo new") }
            let rows = all.map { container -> [String] in
                let state = Archive.archivedAt(container.id).map { ArchiveRetention.summary(archivedAt: $0).lowercased() }
                    ?? container.humanStatus.lowercased()
                return [container.name, container.image, state, container.creator ?? ""]
            }
            // Only show who created what when kuvo new was used.
            let showCreator = all.contains { $0.creator != nil }
            print(CLI.table(["NAME", "IMAGE", "STATE"] + (showCreator ? ["CREATED BY"] : []), rows.map { showCreator ? $0 : Array($0.prefix(3)) }))
        }
    }

    struct Exec: AsyncParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Run a command in a container. Exits with the command's exit code.",
            discussion: "One argument is run as a shell string (pipes and && work). Several are quoted like a normal command line."
        )

        @Option(name: .shortAndLong, help: "Working directory. Default: the container's.") var workdir: String?
        @Argument var container: String
        @Argument(parsing: .captureForPassthrough) var command: [String]

        func run() async throws {
            let parts = command.first == "--" ? Array(command.dropFirst()) : command
            guard !parts.isEmpty else { throw ValidationError("Missing command.") }
            let line = parts.count == 1 ? parts[0] : parts.map(CLI.shellQuoted).joined(separator: " ")

            let service = try await CLI.connect()
            let target = try await service.resolve(container)
            let result = try await service.exec(target, command: line, workdir: workdir) { kind, data in
                (kind == .stderr ? FileHandle.standardError : FileHandle.standardOutput).write(data)
            }
            if let code = result.exitCode, code != 0 { throw ExitCode(Int32(truncatingIfNeeded: code)) }
        }
    }

    struct ShellCommand: AsyncParsableCommand {
        static let configuration = CommandConfiguration(commandName: "shell", abstract: "Open an interactive shell in a container.")

        @Argument var container: String

        func run() async throws {
            let (service, target) = try await CLI.target(container)
            try await service.wake(target)
            let code = try await Shell.run(target, client: service.client)
            if code != 0 { throw ExitCode(code) }
        }
    }

    struct Copy: AsyncParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "cp",
            abstract: "Copy files between your Mac and a container.",
            discussion: """
            kuvo cp <path> <container>[:dir]    Copy a folder or file in (skips git-ignored files)
            kuvo cp <container>:<file> <path>   Copy a file out
            """
        )

        @Argument var source: String
        @Argument var destination: String

        func run() async throws {
            let service = try await CLI.connect()
            if let (name, path) = CLI.remote(source), let path {
                let target = try await service.resolve(name)
                let data = try await service.readFile(target, path: path)
                var url = URL(filePath: (destination as NSString).expandingTildeInPath)
                if FileManager.default.isDirectory(url) { url.append(path: (path as NSString).lastPathComponent) }
                try data.write(to: url)
                print("Copied \(path) to \(url.path.abbreviatingHome) (\(Format.plainBytes(Double(data.count)))).")
            } else {
                let (name, directory) = CLI.remote(destination) ?? (destination, nil)
                let target = try await service.resolve(name)
                let copied = try await service.copyIn(target, hostPath: source, destination: directory)
                let files = copied.files == 1 ? "1 file" : "\(copied.files) files"
                print("Copied \(files) (\(Format.plainBytes(Double(copied.bytes)))) to \(target.name):\(copied.path).")
            }
        }
    }

    struct ArchiveCommand: AsyncParsableCommand {
        static let configuration = CommandConfiguration(commandName: "archive", abstract: "Stop a container and keep it. Deleted after the retention period.")
        @Argument var container: String

        func run() async throws {
            let (service, target) = try await CLI.target(container)
            try await service.archive(target)
            print("Archived \(target.name).")
        }
    }

    struct Restore: AsyncParsableCommand {
        static let configuration = CommandConfiguration(abstract: "Restore an archived container and start it.")
        @Argument var container: String

        func run() async throws {
            let (service, target) = try await CLI.target(container)
            try await service.wake(target)
            print("Restored \(target.name).")
        }
    }

    struct Remove: AsyncParsableCommand {
        static let configuration = CommandConfiguration(commandName: "rm", abstract: "Remove containers now, stopping them first.")
        @Argument var containers: [String]

        func run() async throws {
            let service = try await CLI.connect()
            for name in containers {
                let target = try await service.resolve(name)
                try await service.remove(target)
                print("Removed \(target.name).")
            }
        }
    }

    struct Status: AsyncParsableCommand {
        static let configuration = CommandConfiguration(abstract: "Show whether Kuvo Box's engine is running.")

        func run() async throws {
            let client = DockerClient(socketPath: EnginePaths.socket.path)
            guard let version = try? await client.version() else {
                print("Kuvo Box isn't running. Any kuvo command starts it.")
                throw ExitCode.failure
            }
            let containers = (try? await client.containers()) ?? []
            print("Running: Docker \(version.Version), \(containers.filter(\.isRunning).count) of \(containers.count) containers running.")
            print("Socket: \(EnginePaths.socket.path)")
        }
    }

    struct Env: AsyncParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Print DOCKER_HOST so the docker command uses Kuvo Box.",
            discussion: "eval \"$(kuvo env)\""
        )

        func run() { print("export DOCKER_HOST=unix://\(EnginePaths.socket.path)") }
    }

    // MARK: Helpers

    static func target(_ name: String) async throws -> (ContainerService, Container) {
        let service = try await connect()
        return (service, try await service.resolve(name))
    }

    /// Connects to the engine, starting Kuvo first if needed.
    static func connect() async throws -> ContainerService {
        let service = ContainerService(client: DockerClient(socketPath: EnginePaths.socket.path))
        if (try? await service.client.ping()) != nil { return service }
        if ProcessInfo.processInfo.environment["KUVO_NO_LAUNCH"] == "1" {
            throw DockerError(message: "Kuvo Box's engine isn't running.")
        }

        FileHandle.standardError.write(Data("Starting Kuvo Box…\n".utf8))
        if let app = CLIInstaller.appBundle {
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = false
            _ = try? await NSWorkspace.shared.openApplication(at: app.bundleURL, configuration: configuration)
        } else {
            _ = try? Process.run(URL(filePath: CLIInstaller.executable), arguments: []) // Development builds.
        }
        for _ in 0..<600 {
            try await Task.sleep(for: .milliseconds(500))
            if (try? await service.client.ping()) != nil { return service }
        }
        throw DockerError(message: "The engine didn't start. Open Kuvo Box to see what's wrong.")
    }

    /// "bench:/workspace/out.json" → ("bench", "/workspace/out.json"); local paths → nil.
    static func remote(_ argument: String) -> (String, String?)? {
        guard let colon = argument.firstIndex(of: ":"), !argument.hasPrefix("/"), !argument.hasPrefix("."), !argument.hasPrefix("~")
        else { return nil }
        let path = String(argument[argument.index(after: colon)...])
        return (String(argument[..<colon]), path.isEmpty ? nil : path)
    }

    static func shellQuoted(_ argument: String) -> String {
        guard argument.contains(where: { !$0.isLetter && !$0.isNumber && !"-_./=:@%+,".contains($0) }) else { return argument }
        return "'" + argument.replacingOccurrences(of: "'", with: #"'\''"#) + "'"
    }

    /// "2g", "512m" or "1024" (MB).
    static func megabytes(_ value: String) throws -> Int {
        let lower = value.lowercased()
        let scale: Double = lower.hasSuffix("g") ? 1024 : 1
        let digits = lower.trimmingCharacters(in: CharacterSet(charactersIn: "gmb"))
        guard let amount = Double(digits), amount > 0 else { throw DockerError(message: "\(value) isn't a size like 2g or 512m") }
        return Int(amount * scale)
    }

    static func table(_ header: [String], _ rows: [[String]]) -> String {
        let all = [header] + rows
        let widths = header.indices.map { i in all.map { $0[i].count }.max() ?? 0 }
        return all.map { row in
            row.enumerated().map { i, cell in i == row.count - 1 ? cell : cell.padding(toLength: widths[i], withPad: " ", startingAt: 0) }
                .joined(separator: "  ")
        }.joined(separator: "\n")
    }
}

extension FileManager {
    func isDirectory(_ url: URL) -> Bool {
        var isDirectory: ObjCBool = false
        return fileExists(atPath: url.path, isDirectory: &isDirectory) && isDirectory.boolValue
    }
}
