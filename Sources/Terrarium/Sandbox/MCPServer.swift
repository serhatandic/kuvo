import AppKit
import Foundation

/// `Terrarium mcp`: a Model Context Protocol server over stdio that gives
/// agents sandboxes. Newline-delimited JSON-RPC in, JSON-RPC out.
///
///     claude mcp add terrarium -- /Applications/Terrarium.app/Contents/MacOS/Terrarium mcp
actor MCPServer {
    private var sandboxes = Sandboxes(client: DockerClient(socketPath: EnginePaths.socket.path))
    private var engineReady = false

    static func run() async {
        let server = MCPServer()
        do {
            for try await line in FileHandle.standardInput.bytes.lines where !line.isEmpty {
                Task { await server.handle(line) }
            }
        } catch {}
    }

    // MARK: JSON-RPC

    private func handle(_ line: String) async {
        guard let message = (try? JSONSerialization.jsonObject(with: Data(line.utf8))) as? [String: Any],
              let method = message["method"] as? String else { return }
        let id = message["id"]
        let params = message["params"] as? [String: Any] ?? [:]
        guard id != nil else { return } // Notifications need no reply.

        switch method {
        case "initialize":
            if let name = (params["clientInfo"] as? [String: Any])?["name"] as? String { sandboxes.clientName = name }
            reply(id, result: [
                "protocolVersion": params["protocolVersion"] as? String ?? "2025-06-18",
                "capabilities": ["tools": [:]],
                "serverInfo": ["name": "terrarium", "version": "0.1.0"],
                "instructions": Self.instructions,
            ])
        case "ping":
            reply(id, result: [:])
        case "tools/list":
            reply(id, result: ["tools": Self.tools])
        case "tools/call":
            let name = params["name"] as? String ?? ""
            let arguments = params["arguments"] as? [String: Any] ?? [:]
            do {
                try await ensureEngine()
                reply(id, result: ["content": [["type": "text", "text": try await call(name, arguments)]]])
            } catch {
                reply(id, result: ["content": [["type": "text", "text": error.localizedDescription]], "isError": true])
            }
        default:
            send(["jsonrpc": "2.0", "id": id!, "error": ["code": -32601, "message": "Unknown method \(method)"]])
        }
    }

    private func reply(_ id: Any?, result: [String: Any]) {
        send(["jsonrpc": "2.0", "id": id!, "result": result])
    }

    private func send(_ object: [String: Any]) {
        guard var data = try? JSONSerialization.data(withJSONObject: object, options: [.withoutEscapingSlashes]) else { return }
        data.append(10)
        FileHandle.standardOutput.write(data)
    }

    // MARK: Tools

    private func call(_ tool: String, _ args: [String: Any]) async throws -> String {
        func string(_ key: String) throws -> String {
            guard let value = args[key] as? String, !value.isEmpty else { throw DockerError(message: "Missing \(key)") }
            return value
        }
        func sandbox() async throws -> Container { try await sandboxes.resolve(string("sandbox")) }

        switch tool {
        case "sandbox_create":
            let image = args["image"] as? String ?? Sandboxes.defaultImage
            let created = try await sandboxes.create(
                name: args["name"] as? String, image: image, network: args["network"] as? Bool ?? true,
                cpus: (args["cpus"] as? NSNumber)?.doubleValue, memoryMB: (args["memory_mb"] as? NSNumber)?.intValue
            )
            return "Created sandbox \(created.name) from \(image). Working directory: \(Sandboxes.workspace)."

        case "sandbox_list":
            let all = try await sandboxes.list()
            guard !all.isEmpty else { return "No sandboxes yet. Create one with sandbox_create." }
            return all.map { c in
                let state = Journal.archivedAt(c.id) != nil ? "archived" : c.isRunning ? "running" : "stopped"
                return "\(c.name)  \(c.image)  \(state)  created by \(c.sandboxClient ?? "unknown")"
            }.joined(separator: "\n")

        case "sandbox_exec":
            let timeout = (args["timeout_seconds"] as? NSNumber)?.intValue ?? 600
            let result = try await sandboxes.exec(
                sandbox(), command: string("command"), workdir: args["workdir"] as? String, timeout: .seconds(timeout)
            )
            let status = result.exitCode.map { "Exit code \($0)" } ?? "Still running"
            let output = result.output.isEmpty ? "(no output)" : result.output
            let took = result.duration < 1 ? "\(Int(result.duration * 1000)) ms" : String(format: "%.1f s", result.duration)
            return "\(status) after \(took)\n\n\(output)"

        case "sandbox_write_file":
            let path = try string("path")
            let content = args["content"] as? String ?? ""
            try await sandboxes.writeFile(sandbox(), path: path, content: Data(content.utf8))
            return "Wrote \(content.utf8.count) bytes to \(path)."

        case "sandbox_read_file":
            let data = try await sandboxes.readFile(sandbox(), path: string("path"))
            guard let text = String(data: data, encoding: .utf8) else { return "Binary file, \(data.count) bytes." }
            return data.count > 256 * 1024 ? String(text.prefix(256 * 1024)) + "\n[Truncated at 256 KB]" : text

        case "sandbox_copy_in":
            let copied = try await sandboxes.copyIn(sandbox(), hostPath: string("host_path"), destination: args["destination"] as? String)
            return "Copied \(copied.files) files (\(Format.bytes(Double(copied.bytes)))) to \(copied.path)."

        case "sandbox_archive":
            let target = try await sandbox()
            try await sandboxes.archive(target)
            return "Archived \(target.name). It's stopped and kept; running a command in it restores it."

        case "sandbox_destroy":
            let target = try await sandbox()
            try await sandboxes.destroy(target)
            return "Destroyed \(target.name)."

        default:
            throw DockerError(message: "Unknown tool \(tool)")
        }
    }

    // MARK: Engine

    /// Starts Terrarium if it isn't running, then waits for Docker.
    private func ensureEngine() async throws {
        if engineReady { return }
        if (try? await sandboxes.client.ping()) != nil { engineReady = true; return }

        if Bundle.main.bundleURL.pathExtension == "app" {
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = false
            _ = try? await NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration)
        } else if let executable = Bundle.main.executableURL {
            _ = try? Process.run(executable, arguments: [])
        }
        for _ in 0..<600 {
            try await Task.sleep(for: .milliseconds(500))
            if (try? await sandboxes.client.ping()) != nil { engineReady = true; return }
        }
        throw DockerError(message: "Terrarium's engine didn't start. Open Terrarium to see what's wrong.")
    }

    // MARK: Schema

    private static let instructions = """
    Terrarium gives you disposable Linux sandboxes: Docker containers in a local VM, isolated from the user's Mac. \
    Use them for throwaway work like benchmarks, experiments and installing tools. \
    Sandboxes persist until archived or destroyed, so check sandbox_list and reuse one when it fits. \
    The working directory is /workspace. The network is on unless you turn it off. \
    The user's files are not inside unless you copy them in with sandbox_copy_in.
    """

    private static var sandboxParam: [String: Any] { ["type": "string", "description": "Sandbox name (or ID)."] }

    private static var tools: [[String: Any]] { [
        tool("sandbox_create", "Create a new sandbox and start it.", [
            "name": ["type": "string", "description": "Optional name, e.g. \"bench-json\". Generated if omitted."],
            "image": ["type": "string", "description": "Docker image. Default: \(Sandboxes.defaultImage)."],
            "network": ["type": "boolean", "description": "Internet access. Default: true."],
            "cpus": ["type": "number", "description": "CPU limit, e.g. 2. Default: unlimited."],
            "memory_mb": ["type": "integer", "description": "Memory limit in MB. Default: unlimited."],
        ]),
        tool("sandbox_list", "List sandboxes and their state.", [:]),
        tool("sandbox_exec", "Run a shell command in a sandbox (bash if available) and return its output and exit code.", [
            "sandbox": sandboxParam,
            "command": ["type": "string", "description": "Shell command, e.g. \"apt-get update && apt-get install -y hyperfine\"."],
            "workdir": ["type": "string", "description": "Working directory. Default: /workspace."],
            "timeout_seconds": ["type": "integer", "description": "Stop waiting after this long (the command keeps running). Default: 600."],
        ], required: ["sandbox", "command"]),
        tool("sandbox_write_file", "Create or overwrite a text file in a sandbox.", [
            "sandbox": sandboxParam,
            "path": ["type": "string", "description": "Absolute, or relative to /workspace."],
            "content": ["type": "string"],
        ], required: ["sandbox", "path", "content"]),
        tool("sandbox_read_file", "Read a file from a sandbox.", [
            "sandbox": sandboxParam,
            "path": ["type": "string", "description": "Absolute, or relative to /workspace."],
        ], required: ["sandbox", "path"]),
        tool("sandbox_copy_in", "Copy a folder or file from the user's Mac into a sandbox. Respects .gitignore. Only do this when the task needs the user's code.", [
            "sandbox": sandboxParam,
            "host_path": ["type": "string", "description": "Path on the Mac, e.g. ~/code/project."],
            "destination": ["type": "string", "description": "Directory in the sandbox to copy into. Default: /workspace."],
        ], required: ["sandbox", "host_path"]),
        tool("sandbox_archive", "Stop a sandbox and keep it for later. Archived sandboxes are deleted after a retention period.", [
            "sandbox": sandboxParam,
        ], required: ["sandbox"]),
        tool("sandbox_destroy", "Delete a sandbox and everything in it, now.", [
            "sandbox": sandboxParam,
        ], required: ["sandbox"]),
    ] }

    private static func tool(_ name: String, _ description: String, _ properties: [String: Any], required: [String] = []) -> [String: Any] {
        ["name": name, "description": description,
         "inputSchema": ["type": "object", "properties": properties, "required": required]]
    }
}
