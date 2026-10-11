import CryptoKit
import Foundation

/// Points the docker command at Kuvo: creates a `kuvo` Docker context and makes it
/// current, writing the same files `docker context create` and `docker context use`
/// do. Written directly, so it works when the docker command isn't installed yet or
/// isn't on the app's PATH.
enum DockerContext {
    static let name = "kuvo"
    private static let askedKey = "dockerContext.asked"
    /// ~/.docker, or wherever DOCKER_CONFIG points, as the docker command does.
    private static var dockerFolder: URL {
        if let custom = ProcessInfo.processInfo.environment["DOCKER_CONFIG"], !custom.isEmpty {
            return URL(filePath: custom, directoryHint: .isDirectory)
        }
        return URL.homeDirectory.appending(path: ".docker", directoryHint: .isDirectory)
    }
    private static var configURL: URL { dockerFolder.appending(path: "config.json") }

    /// The context the docker command uses now ("default" when none is set).
    static var current: String {
        guard let data = try? Data(contentsOf: configURL),
              let config = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return "default" }
        return config["currentContext"] as? String ?? "default"
    }

    static var hasDockerDesktop: Bool {
        FileManager.default.fileExists(atPath: "/Applications/Docker.app")
    }

    /// Asked once, the first time the engine is up, unless docker already uses Kuvo.
    static var shouldAsk: Bool {
        !UserDefaults.standard.bool(forKey: askedKey) && current != name
    }

    static func markAsked() {
        UserDefaults.standard.set(true, forKey: askedKey)
    }

    static func use() throws {
        let fm = FileManager.default

        // Read config.json first, so a file Kuvo can't parse is left exactly as it was.
        var config: [String: Any] = [:]
        var attributes: [FileAttributeKey: Any]?
        if fm.fileExists(atPath: configURL.path) {
            guard let parsed = try? JSONSerialization.jsonObject(with: Data(contentsOf: configURL)) as? [String: Any] else {
                throw DockerError(message: "\(configURL.path) isn't valid JSON, so Kuvo left it alone. Fix it, then run: docker context use kuvo")
            }
            config = parsed
            attributes = try? fm.attributesOfItem(atPath: configURL.path)
        }

        // The context: contexts/meta/<sha256 of its name>/meta.json.
        let hash = SHA256.hash(data: Data(name.utf8)).map { String(format: "%02x", $0) }.joined()
        let meta = dockerFolder.appending(path: "contexts/meta/\(hash)", directoryHint: .isDirectory)
        try fm.createDirectory(at: meta, withIntermediateDirectories: true)
        let context: [String: Any] = [
            "Name": name,
            "Metadata": [String: Any](),
            "Endpoints": ["docker": ["Host": "unix://\(EnginePaths.socket.path)", "SkipTLSVerify": false]],
        ]
        try JSONSerialization.data(withJSONObject: context, options: .withoutEscapingSlashes)
            .write(to: meta.appending(path: "meta.json"), options: .atomic)

        // Make it current, keeping everything else in config.json (logins, plugins)
        // and the file's permissions as they were.
        config["currentContext"] = name
        try JSONSerialization.data(withJSONObject: config, options: [.prettyPrinted, .withoutEscapingSlashes])
            .write(to: configURL, options: .atomic)
        if let permissions = attributes?[.posixPermissions] {
            try? fm.setAttributes([.posixPermissions: permissions], ofItemAtPath: configURL.path)
        }
    }
}
