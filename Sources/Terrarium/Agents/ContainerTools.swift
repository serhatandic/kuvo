import Foundation

/// Everything you can see and do in the app, as plain text an agent can read.
struct ContainerTools: Sendable {
    let client: DockerClient

    // MARK: Read

    func list() async throws -> String {
        let containers = try await client.containers()
        guard !containers.isEmpty else { return "No containers." }
        let rows = containers.map { c in
            [c.name, c.image, c.isSandbox ? "\(c.state.rawValue) (sandbox)" : c.state.rawValue, c.status,
             c.ports.compactMap { p in p.publicPort.map { "\($0)->\(p.privatePort)" } }.uniqued.joined(separator: " "),
             c.project ?? ""]
        }
        return table(["NAME", "IMAGE", "STATE", "STATUS", "PORTS", "PROJECT"], rows)
    }

    func inspect(_ reference: String) async throws -> String {
        let container = try await resolve(reference)
        let data = try await client.send("GET", "/containers/\(container.id)/json")
        guard let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { return "No details." }
        let state = json["State"] as? [String: Any] ?? [:]
        let config = json["Config"] as? [String: Any] ?? [:]
        let host = json["HostConfig"] as? [String: Any] ?? [:]
        let network = json["NetworkSettings"] as? [String: Any] ?? [:]

        var lines = ["name: \(container.name)", "id: \(container.shortID)", "image: \(container.image)",
                     "state: \(state["Status"] ?? "unknown")"]
        if let health = state["Health"] as? [String: Any] {
            lines.append("health: \(health["Status"] ?? "")")
            if let last = (health["Log"] as? [[String: Any]])?.last, let output = last["Output"] as? String, !output.isEmpty {
                lines.append("last health check: \(output.trimmingCharacters(in: .whitespacesAndNewlines).prefix(300))")
            }
        }
        lines.append("started: \(state["StartedAt"] ?? "")")
        if container.state == .exited || container.state == .dead {
            lines.append("finished: \(state["FinishedAt"] ?? "")")
            lines.append("exit code: \(state["ExitCode"] ?? "")")
        }
        if state["OOMKilled"] as? Bool == true { lines.append("killed: out of memory") }
        if let error = state["Error"] as? String, !error.isEmpty { lines.append("error: \(error)") }
        lines.append("restarts: \(json["RestartCount"] ?? 0), policy: \((host["RestartPolicy"] as? [String: Any])?["Name"] ?? "no")")

        let command = ((config["Entrypoint"] as? [String]) ?? []) + ((config["Cmd"] as? [String]) ?? [])
        if !command.isEmpty { lines.append("command: \(command.joined(separator: " "))") }
        if let memory = host["Memory"] as? Int, memory > 0 { lines.append("memory limit: \(Format.plainBytes(Double(memory)))") }
        if let cpus = host["NanoCpus"] as? Int, cpus > 0 { lines.append("cpu limit: \(Double(cpus) / 1e9)") }
        if let project = container.project { lines.append("compose: project \(project), service \(container.service ?? "")") }

        if let ports = network["Ports"] as? [String: Any], !ports.isEmpty {
            lines.append("ports:")
            for (port, bindings) in ports.sorted(by: { $0.key < $1.key }) {
                let host = (bindings as? [[String: Any]])?.compactMap { $0["HostPort"] as? String }.uniqued.joined(separator: ", ")
                lines.append("  \(port)" + (host.map { $0.isEmpty ? "" : " <- localhost:\($0)" } ?? " (not published)"))
            }
        }
        if let networks = network["Networks"] as? [String: [String: Any]], !networks.isEmpty {
            lines.append("networks: " + networks.map { "\($0.key) (\($0.value["IPAddress"] as? String ?? ""))" }.sorted().joined(separator: ", "))
        }
        if let mounts = json["Mounts"] as? [[String: Any]], !mounts.isEmpty {
            lines.append("mounts:")
            for mount in mounts {
                let source = mount["Name"] as? String ?? mount["Source"] as? String ?? ""
                lines.append("  \(source) -> \(mount["Destination"] ?? "")\(mount["RW"] as? Bool == false ? " (read-only)" : "")")
            }
        }
        if let env = config["Env"] as? [String], !env.isEmpty {
            lines.append("env:")
            lines += env.sorted().map { "  " + Self.redacted($0) }
        }
        return lines.joined(separator: "\n")
    }

    func logs(_ reference: String, tail: Int, since: String?, grep: String?) async throws -> String {
        let container = try await resolve(reference)
        let tail = min(max(tail, 1), 2000)
        var query = "stdout=true&stderr=true&timestamps=true&tail=\(grep == nil ? tail : 10_000)"
        if let since { query += "&since=\(try Self.since(since))" }

        var decoder = LogDecoder()
        var lines: [LogLine] = []
        for try await chunk in client.stream("GET", "/containers/\(container.id)/logs?\(query)") {
            lines += decoder.feed(chunk)
        }
        if let grep {
            let pattern = try? Regex(grep).ignoresCase()
            lines = lines.filter { line in
                pattern.map { line.text.contains($0) } ?? line.text.localizedCaseInsensitiveContains(grep)
            }
        }
        lines = Array(lines.suffix(tail))
        guard !lines.isEmpty else { return grep == nil ? "No log output." : "No lines match \(grep!)." }
        let text = lines.map { line in
            let time = line.date.map { $0.formatted(.iso8601) + " " } ?? ""
            return time + line.text.strippingANSI
        }.joined(separator: "\n")
        return text.count > 100_000 ? "[…]\n" + String(text.suffix(100_000)) : text
    }

    func stats(_ reference: String?) async throws -> String {
        let targets = if let reference { [try await resolve(reference)] } else { try await client.containers().filter(\.isRunning) }
        guard !targets.isEmpty else { return "No running containers." }
        let samples = await withTaskGroup(of: (Container, StatsSample?).self) { group in
            for container in targets {
                group.addTask {
                    let data = try? await client.send("GET", "/containers/\(container.id)/stats?stream=false")
                    return (container, data.flatMap { try? JSONDecoder().decode(StatsSample.self, from: $0) })
                }
            }
            var results: [(Container, StatsSample?)] = []
            for await result in group { results.append(result) }
            return results.sorted { $0.0.name < $1.0.name }
        }
        let rows = samples.map { container, sample -> [String] in
            guard let s = sample else { return [container.name, "-", "-", "-"] }
            return [container.name, Format.percent(s.cpuPercent),
                    "\(Format.plainBytes(Double(s.memoryUsed))) / \(Format.plainBytes(Double(s.memoryLimit)))",
                    "\(Format.plainBytes(Double(s.rxBytes))) / \(Format.plainBytes(Double(s.txBytes)))"]
        }
        return table(["NAME", "CPU (OF ONE CORE)", "MEMORY / LIMIT", "NET RX / TX (TOTAL)"], rows)
    }

    func images() async throws -> String {
        struct Image: Decodable { let RepoTags: [String]?; let Size: Int; let Created: Int }
        let images = try await JSONDecoder().decode([Image].self, from: client.send("GET", "/images/json"))
        guard !images.isEmpty else { return "No images." }
        let rows = images.sorted { $0.Created > $1.Created }.map { image in
            [(image.RepoTags ?? ["<none>"]).joined(separator: ", "), Format.plainBytes(Double(image.Size)),
             Date(timeIntervalSince1970: Double(image.Created)).formatted(.relative(presentation: .named))]
        }
        return table(["IMAGE", "SIZE", "CREATED"], rows)
    }

    // MARK: Act

    func exec(_ reference: String, command: String, workdir: String?, timeout: Duration) async throws -> ExecResult {
        let container = try await resolve(reference)
        guard container.state == .running else { throw DockerError(message: "\(container.name) isn't running. Start it first.") }
        return try await client.execute(in: container.id, command: command, workdir: workdir, timeout: timeout)
    }

    func perform(_ action: ContainerAction, on reference: String) async throws -> String {
        let container = try await resolve(reference)
        try await client.perform(action, on: container.id)
        return "\(action.pastTense) \(container.name)."
    }

    func perform(_ action: ContainerAction, project: String) async throws -> String {
        let members = try await client.containers().filter { $0.project == project }
        guard !members.isEmpty else { throw DockerError(message: "No Compose project named \(project).") }
        var done: [String] = []
        for container in members {
            let applies = switch action {
            case .start: !container.isRunning
            case .stop, .restart, .pause: container.isRunning
            case .unpause: container.state == .paused
            case .remove: true
            }
            guard applies else { continue }
            try await client.perform(action, on: container.id)
            done.append(container.service ?? container.name)
        }
        return done.isEmpty ? "Nothing to do in \(project)." : "\(action.pastTense) \(done.joined(separator: ", ")) in \(project)."
    }

    func run(image: String, name: String?, ports: [String], env: [String: String], command: [String]?) async throws -> String {
        try await client.pull(image)
        var exposed: [String: Any] = [:]
        var bindings: [String: Any] = [:]
        for mapping in ports {
            // "8080:80", "8080:80/udp" or "80"
            let parts = mapping.split(separator: ":").map(String.init)
            let container = parts.last!.contains("/") ? parts.last! : "\(parts.last!)/tcp"
            exposed[container] = [String: Any]()
            bindings[container] = [["HostPort": parts.count > 1 ? parts[0].replacingOccurrences(of: "/tcp", with: "") : ""]]
        }
        var config: [String: Any] = [
            "Image": image,
            "Env": env.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" },
            "ExposedPorts": exposed,
            "HostConfig": ["PortBindings": bindings],
        ]
        if let command, !command.isEmpty { config["Cmd"] = command }
        struct Created: Decodable { let Id: String }
        let path = name.map { "/containers/create?name=\($0.urlQuery)" } ?? "/containers/create"
        let created = try await client.send("POST", path, json: config, as: Created.self)
        try await client.perform(.start, on: created.Id)
        let container = try await resolve(created.Id)
        let published = container.publishedPorts.map { "localhost:\($0)" }.joined(separator: ", ")
        return "Started \(container.name) from \(image)." + (published.isEmpty ? "" : " Reachable at \(published).")
    }

    // MARK: Helpers

    /// By name, Compose service name (when unambiguous), or ID prefix.
    func resolve(_ reference: String) async throws -> Container {
        let all = try await client.containers()
        if let exact = all.first(where: { $0.name == reference || $0.id == reference }) { return exact }
        let matches = all.filter { $0.service == reference || (reference.count >= 3 && $0.id.hasPrefix(reference)) }
        if matches.count == 1 { return matches[0] }
        if matches.count > 1 {
            throw DockerError(message: "\(reference) is ambiguous: \(matches.map(\.name).joined(separator: ", ")).")
        }
        throw DockerError(message: "No container named \(reference). Use containers_list to see what exists.")
    }

    /// Env values whose names look secret are hidden from transcripts.
    static func redacted(_ variable: String) -> String {
        guard let equals = variable.firstIndex(of: "=") else { return variable }
        let key = variable[..<equals].uppercased()
        let secret = ["PASSWORD", "PASSWD", "SECRET", "TOKEN", "KEY", "CREDENTIAL", "AUTH", "PRIVATE"].contains { key.contains($0) }
        return secret ? "\(variable[..<equals])=<redacted>" : variable
    }

    /// "10m", "2h", "1d" or "30s" become a Unix timestamp; anything else passes through.
    private static func since(_ value: String) throws -> String {
        guard let match = value.wholeMatch(of: /(\d+)([smhd])/), let amount = Double(match.1) else { return value.urlQuery }
        let unit: Double = ["s": 1, "m": 60, "h": 3600, "d": 86_400][String(match.2)]!
        return String(Int(Date().timeIntervalSince1970 - amount * unit))
    }

    private func table(_ header: [String], _ rows: [[String]]) -> String {
        let all = [header] + rows
        let widths = header.indices.map { i in all.map { $0[i].count }.max() ?? 0 }
        return all.map { row in
            row.enumerated().map { i, cell in i == row.count - 1 ? cell : cell.padding(toLength: widths[i], withPad: " ", startingAt: 0) }
                .joined(separator: "  ").trimmingCharacters(in: .whitespaces)
        }.joined(separator: "\n")
    }
}

extension ContainerAction {
    var pastTense: String {
        switch self {
        case .start: "Started"
        case .stop: "Stopped"
        case .restart: "Restarted"
        case .pause: "Paused"
        case .unpause: "Resumed"
        case .remove: "Removed"
        }
    }
}

extension String {
    /// Removes terminal color and cursor codes.
    var strippingANSI: String { replacing(/\u{1B}\[[0-9;?]*[A-Za-z]/, with: "") }
}

extension Array where Element: Hashable {
    var uniqued: [Element] {
        var seen = Set<Element>()
        return filter { seen.insert($0).inserted }
    }
}
