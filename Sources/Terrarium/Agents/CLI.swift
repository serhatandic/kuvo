import AppKit
import ArgumentParser
import Foundation

/// `terrarium`: disposable Linux sandboxes from the command line, written to
/// read well for people and for agents (plain output, real exit codes).
struct CLI: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "terrarium",
        abstract: "Disposable Linux sandboxes, isolated from your Mac.",
        discussion: """
        Sandboxes keep their state until you archive or remove them, so reuse one when it fits.
        Commands run in /workspace with bash. The network is on unless you turn it off.

        Examples:
          terrarium new bench --cpus 2 --memory 2g
          terrarium exec bench "apt-get update && apt-get install -y hyperfine"
          terrarium cp ~/code/parser bench
          terrarium exec -w /workspace/parser bench hyperfine 'python3 bench.py'
        """,
        version: CLIInstaller.appBundle?.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev",
        subcommands: [New.self, List.self, Exec.self, ShellCommand.self, Copy.self,
                      Archive.self, Restore.self, Remove.self, Status.self, Env.self]
    )

    struct New: AsyncParsableCommand {
        static let configuration = CommandConfiguration(abstract: "Create a sandbox and start it.")

        @Argument(help: "Name, e.g. bench. Generated if omitted.") var name: String?
        @Option(help: "Docker image.") var image = Sandboxes.defaultImage
        @Flag(help: "Turn off network access.") var noNetwork = false
        @Option(help: "CPU limit, e.g. 2.") var cpus: Double?
        @Option(help: "Memory limit, e.g. 2g or 512m.") var memory: String?

        func run() async throws {
            let sandbox = try await CLI.connect().create(
                name: name, image: image, network: !noNetwork, cpus: cpus, memoryMB: try memory.map(CLI.megabytes)
            )
            print("Created sandbox \(sandbox.name) from \(image).")
            print("Run commands with: terrarium exec \(sandbox.name) <command>")
        }
    }

    struct List: AsyncParsableCommand {
        static let configuration = CommandConfiguration(commandName: "ls", abstract: "List sandboxes.")

        func run() async throws {
            let all = try await CLI.connect().list()
            guard !all.isEmpty else { return print("No sandboxes. Create one with: terrarium new") }
            let rows = all.sorted { $0.created > $1.created }.map { sandbox -> [String] in
                let state = Journal.archivedAt(sandbox.id).map { SandboxRetention.summary(archivedAt: $0).lowercased() }
                    ?? (sandbox.isRunning ? "running" : "stopped")
                return [sandbox.name, sandbox.image, state, sandbox.created.formatted(.relative(presentation: .named)),
                        sandbox.sandboxClient ?? ""]
            }
            print(CLI.table(["NAME", "IMAGE", "STATE", "CREATED", "BY"], rows))
        }
    }

    struct Exec: AsyncParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Run a command in a sandbox. Exits with the command's exit code.",
            discussion: "One argument is run as a shell string (pipes and && work). Several are quoted like a normal command line."
        )

        @Option(name: .shortAndLong, help: "Working directory. Default: /workspace.") var workdir: String?
        @Argument var sandbox: String
        @Argument(parsing: .captureForPassthrough) var command: [String]

        func run() async throws {
            let parts = command.first == "--" ? Array(command.dropFirst()) : command
            guard !parts.isEmpty else { throw ValidationError("Missing command.") }
            let line = parts.count == 1 ? parts[0] : parts.map(CLI.shellQuoted).joined(separator: " ")

            let sandboxes = try await CLI.connect()
            let target = try await sandboxes.resolve(sandbox)
            let interrupt = Interrupt(sandbox: target.id)
            let result = try await sandboxes.exec(target, command: line, workdir: workdir, onOutput: { kind, data in
                (kind == .stderr ? FileHandle.standardError : FileHandle.standardOutput).write(data)
            }, started: interrupt.arm)
            if let code = result.exitCode, code != 0 { throw ExitCode(Int32(truncatingIfNeeded: code)) }
        }
    }

    struct ShellCommand: AsyncParsableCommand {
        static let configuration = CommandConfiguration(commandName: "shell", abstract: "Open an interactive shell in a sandbox.")

        @Argument var sandbox: String

        func run() async throws {
            let (sandboxes, target) = try await CLI.target(sandbox)
            try await sandboxes.wake(target)
            let code = try await Shell.run(target, client: sandboxes.client)
            if code != 0 { throw ExitCode(code) }
        }
    }

    struct Copy: AsyncParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "cp",
            abstract: "Copy files between your Mac and a sandbox.",
            discussion: """
            terrarium cp <path> <sandbox>[:dir]    Copy a folder or file in (skips git-ignored files)
            terrarium cp <sandbox>:<file> <path>   Copy a file out
            """
        )

        @Argument var source: String
        @Argument var destination: String

        func run() async throws {
            let sandboxes = try await CLI.connect()
            if let (name, path) = CLI.remote(source), let path {
                let target = try await sandboxes.resolve(name)
                let data = try await sandboxes.readFile(target, path: path)
                var url = URL(filePath: (destination as NSString).expandingTildeInPath)
                if FileManager.default.isDirectory(url) { url.append(path: (path as NSString).lastPathComponent) }
                try data.write(to: url)
                print("Copied \(path) to \(url.path.abbreviatingHome) (\(Format.plainBytes(Double(data.count)))).")
            } else {
                let (name, directory) = CLI.remote(destination) ?? (destination, nil)
                let target = try await sandboxes.resolve(name)
                let copied = try await sandboxes.copyIn(target, hostPath: source, destination: directory)
                print("Copied \(copied.files) files (\(Format.plainBytes(Double(copied.bytes)))) to \(target.name):\(copied.path).")
            }
        }
    }

    struct Archive: AsyncParsableCommand {
        static let configuration = CommandConfiguration(abstract: "Stop a sandbox and keep it. Deleted after the retention period.")
        @Argument var sandbox: String

        func run() async throws {
            let (sandboxes, target) = try await CLI.target(sandbox)
            try await sandboxes.archive(target)
            print("Archived \(target.name).")
        }
    }

    struct Restore: AsyncParsableCommand {
        static let configuration = CommandConfiguration(abstract: "Restore an archived sandbox and start it.")
        @Argument var sandbox: String

        func run() async throws {
            let (sandboxes, target) = try await CLI.target(sandbox)
            try await sandboxes.wake(target)
            print("Restored \(target.name).")
        }
    }

    struct Remove: AsyncParsableCommand {
        static let configuration = CommandConfiguration(commandName: "rm", abstract: "Delete sandboxes now.")
        @Argument var sandboxes: [String]

        func run() async throws {
            let client = try await CLI.connect()
            for name in sandboxes {
                let target = try await client.resolve(name)
                try await client.destroy(target)
                print("Removed \(target.name).")
            }
        }
    }

    struct Status: AsyncParsableCommand {
        static let configuration = CommandConfiguration(abstract: "Show whether Terrarium's engine is running.")

        func run() async throws {
            let client = DockerClient(socketPath: EnginePaths.socket.path)
            guard let version = try? await client.version() else {
                print("Terrarium isn't running. Any terrarium command starts it.")
                throw ExitCode.failure
            }
            let containers = (try? await client.containers()) ?? []
            let sandboxes = containers.filter(\.isSandbox).count
            let running = containers.filter { $0.isRunning && !$0.isSandbox }.count
            print("Running: Docker \(version.Version), \(running) containers running, \(sandboxes) sandboxes.")
            print("Socket: \(EnginePaths.socket.path)")
        }
    }

    struct Env: AsyncParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Print DOCKER_HOST so the docker command uses Terrarium.",
            discussion: "eval \"$(terrarium env)\""
        )

        func run() { print("export DOCKER_HOST=unix://\(EnginePaths.socket.path)") }
    }

    // MARK: Helpers

    static func target(_ name: String) async throws -> (Sandboxes, Container) {
        let sandboxes = try await connect()
        return (sandboxes, try await sandboxes.resolve(name))
    }

    /// Connects to the engine, starting Terrarium first if needed.
    static func connect() async throws -> Sandboxes {
        let sandboxes = Sandboxes(client: DockerClient(socketPath: EnginePaths.socket.path))
        if (try? await sandboxes.client.ping()) != nil { return sandboxes }

        FileHandle.standardError.write(Data("Starting Terrarium…\n".utf8))
        if let app = CLIInstaller.appBundle {
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = false
            _ = try? await NSWorkspace.shared.openApplication(at: app.bundleURL, configuration: configuration)
        } else {
            _ = try? Process.run(URL(filePath: CLIInstaller.executable), arguments: []) // Development builds.
        }
        for _ in 0..<600 {
            try await Task.sleep(for: .milliseconds(500))
            if (try? await sandboxes.client.ping()) != nil { return sandboxes }
        }
        throw DockerError(message: "The engine didn't start. Open Terrarium to see what's wrong.")
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

/// Closes the journal record if the user presses Ctrl-C mid-command.
final class Interrupt: @unchecked Sendable {
    private let sandbox: String
    private var source: DispatchSourceSignal?

    init(sandbox: String) { self.sandbox = sandbox }

    func arm(_ entry: Journal.Entry) {
        signal(SIGINT, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
        source.setEventHandler { [sandbox] in
            var done = entry
            done.finish(exitCode: 130, duration: Date().timeIntervalSince(entry.date), output: "[Interrupted]")
            Journal.append(done, to: sandbox)
            exit(130)
        }
        source.resume()
        self.source = source
    }
}

extension FileManager {
    func isDirectory(_ url: URL) -> Bool {
        var isDirectory: ObjCBool = false
        return fileExists(atPath: url.path, isDirectory: &isDirectory) && isDirectory.boolValue
    }
}
