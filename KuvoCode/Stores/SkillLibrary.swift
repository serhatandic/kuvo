import AppKit
import Foundation

/// Skills from public GitHub repositories: the popular collections to choose from, the skills
/// in each, and installing one into the engine's skills folder (where every project can use
/// it). Nothing is fetched until you open a collection.
nonisolated enum SkillLibrary {
    struct Repository: Hashable, Identifiable, Sendable {
        var owner: String
        var name: String
        var title: String
        var summary: String
        var branch = "main"

        var id: String { "\(owner)/\(name)" }
        var webURL: URL { URL(string: "https://github.com/\(owner)/\(name)")! }
    }

    /// Well-known collections, roughly most popular first. Anyone can add another by name.
    static let repositories: [Repository] = [
        Repository(owner: "anthropics", name: "skills", title: "Anthropic", summary: "The reference set: documents, spreadsheets, slides, PDFs, design and more."),
        Repository(owner: "openai", name: "skills", title: "OpenAI", summary: "Codex's catalog of curated skills."),
        Repository(owner: "obra", name: "superpowers", title: "Superpowers", summary: "Ways of working: test-driven development, debugging, planning, code review."),
        Repository(owner: "mattpocock", name: "skills", title: "Matt Pocock", summary: "Engineering and productivity skills, TypeScript-minded."),
        Repository(owner: "addyosmani", name: "agent-skills", title: "Addy Osmani", summary: "Production engineering practices: performance, accessibility, testing."),
        Repository(owner: "vercel-labs", name: "agent-skills", title: "Vercel", summary: "React, Next.js and deploying to Vercel."),
        Repository(owner: "github", name: "awesome-copilot", title: "GitHub Awesome Copilot", summary: "Hundreds of community skills; quality varies."),
        Repository(owner: "huggingface", name: "skills", title: "Hugging Face", summary: "Models, datasets and training with Hugging Face."),
        Repository(owner: "cloudflare", name: "skills", title: "Cloudflare", summary: "Workers, Pages and the rest of Cloudflare's platform."),
        Repository(owner: "trailofbits", name: "skills", title: "Trail of Bits", summary: "Security auditing and code review."),
        Repository(owner: "K-Dense-AI", name: "scientific-agent-skills", title: "Scientific Skills", summary: "A large library for science: biology, chemistry, data analysis."),
        Repository(owner: "wshobson", name: "agents", title: "wshobson/agents", summary: "A big set of plugins with skills for many languages and tools."),
    ]

    /// A skill in a repository: the folder its SKILL.md is in.
    struct RemoteSkill: Hashable, Identifiable, Sendable {
        var repository: Repository
        /// The folder in the repository, like `skills/pdf`.
        var path: String
        /// Every file in it, relative to the repository.
        var files: [String]

        /// Unique in its repository; two folders can share a name.
        var id: String { path }
        /// The engine knows a skill by its folder's name, and it's installed under it: the
        /// repository's, for one that's a skill itself (its SKILL.md at the top).
        var name: String { path.isEmpty ? repository.name : (path as NSString).lastPathComponent }

        /// Its SKILL.md, in the repository.
        var skillFile: String { path.isEmpty ? "SKILL.md" : path + "/SKILL.md" }

        /// A file of it, relative to its folder.
        func relative(_ file: String) -> String { path.isEmpty ? file : String(file.dropFirst(path.count + 1)) }
    }

    /// A repository's skills, and whether GitHub cut the list short (a very big repository).
    struct Listing: Sendable {
        var skills: [RemoteSkill]
        var isPartial: Bool
    }

    /// Where an installed skill came from.
    struct Source: Sendable, Hashable {
        var repository: String
        var path: String
    }

    /// What a SKILL.md says about itself.
    struct About: Sendable {
        var name: String?
        var description: String?
    }

    enum Failure: LocalizedError {
        case notFound(String)
        case limited
        case taken(String)

        var errorDescription: String? {
            switch self {
            case .notFound(let repository): "\(repository) wasn't found on GitHub."
            case .limited: "GitHub is limiting requests for now. Try again in a while."
            case .taken(let id): "A skill named \(id) is already installed."
            }
        }
    }

    /// A repository you name ("owner/name", or its GitHub URL), with its default branch.
    @concurrent static func repository(named text: String) async throws -> Repository {
        var name = text.trimmingCharacters(in: .whitespacesAndNewlines)
        for prefix in ["https://github.com/", "http://github.com/", "github.com/"] where name.hasPrefix(prefix) { name.removeFirst(prefix.count) }
        let parts = name.split(separator: "/").prefix(2).map(String.init)
        guard parts.count == 2 else { throw Failure.notFound(text) }
        struct Info: Decodable { var default_branch: String; var description: String? }
        let (data, response) = try await URLSession.shared.data(from: URL(string: "https://api.github.com/repos/\(parts[0])/\(parts[1])")!)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if status == 403 || status == 429 { throw Failure.limited }
        guard status == 200, let info = try? JSONDecoder().decode(Info.self, from: data) else { throw Failure.notFound(name) }
        return Repository(owner: parts[0], name: parts[1], title: name, summary: info.description ?? "", branch: info.default_branch)
    }

    /// The skills in a repository, from its file list (one request to GitHub's API).
    @concurrent static func skills(in repository: Repository) async throws -> Listing {
        struct Tree: Decodable {
            var tree: [Entry]
            var truncated: Bool?
            struct Entry: Decodable { var path: String; var type: String }
        }
        let url = URL(string: "https://api.github.com/repos/\(repository.owner)/\(repository.name)/git/trees/\(repository.branch)?recursive=1")!
        var request = URLRequest(url: url)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        let (data, response) = try await URLSession.shared.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if status == 403 || status == 429 { throw Failure.limited }
        guard status == 200 else { throw Failure.notFound(repository.id) }
        let tree = try JSONDecoder().decode(Tree.self, from: data)
        let files = tree.tree.filter { $0.type == "blob" }.map(\.path)
        let folders = Set(files.filter { ($0 as NSString).lastPathComponent == "SKILL.md" }.map { ($0 as NSString).deletingLastPathComponent }.filter { !$0.isEmpty })
        // A repository that's one skill: its SKILL.md at the top, with what's beside it (not its
        // .github and other hidden folders).
        let isSkill = files.contains("SKILL.md")
        // Each file to the skill folder it's in (the deepest one, for skills inside skills).
        var byFolder: [String: [String]] = [:]
        for file in files {
            var folder = (file as NSString).deletingLastPathComponent
            while !folder.isEmpty, !folders.contains(folder) { folder = (folder as NSString).deletingLastPathComponent }
            if !folder.isEmpty { byFolder[folder, default: []].append(file) }
            else if isSkill, !file.hasPrefix(".") { byFolder["", default: []].append(file) }
        }
        var skills = ((isSkill ? [""] : []) + folders.sorted()).map { RemoteSkill(repository: repository, path: $0, files: byFolder[$0] ?? []) }
        // A copy kept for another tool, in a hidden folder (.claude/skills, .openclaw/skills…),
        // of a skill the repository has in plain sight: listed once, as the one in plain sight.
        func isHidden(_ skill: RemoteSkill) -> Bool { skill.path.split(separator: "/").contains { $0.hasPrefix(".") } }
        let shown = Set(skills.filter { !isHidden($0) }.map(\.name))
        skills.removeAll { isHidden($0) && shown.contains($0.name) }
        return Listing(skills: skills, isPartial: tree.truncated == true)
    }

    /// The name and description in a skill's SKILL.md.
    @concurrent static func about(_ skill: RemoteSkill) async -> About {
        guard let data = try? await URLSession.shared.data(from: rawURL(skill.repository, skill.skillFile)).0 else { return About() }
        return about(markdown: String(decoding: data, as: UTF8.self))
    }

    /// The front matter's `name` and `description`.
    static func about(markdown: String) -> About {
        let lines = markdown.components(separatedBy: .newlines)
        guard lines.first?.trimmingCharacters(in: .whitespaces) == "---" else { return About() }
        var about = About()
        var index = 1
        while index < lines.count, lines[index].trimmingCharacters(in: .whitespaces) != "---" {
            let line = lines[index]
            if let colon = line.firstIndex(of: ":"), !line.hasPrefix(" ") {
                let key = line[..<colon].trimmingCharacters(in: .whitespaces)
                var value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
                // A folded or literal block: the indented lines after it.
                if value == ">" || value == "|" || value == ">-" || value == "|-" {
                    var parts: [String] = []
                    while index + 1 < lines.count, lines[index + 1].hasPrefix(" ") {
                        index += 1
                        parts.append(lines[index].trimmingCharacters(in: .whitespaces))
                    }
                    value = parts.joined(separator: " ")
                }
                value = value.trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
                if key == "name" { about.name = value }
                if key == "description" { about.description = value }
            }
            index += 1
        }
        return about
    }

    /// Copies a skill's files into `folder/<name>`, with a note of where they came from.
    @concurrent static func install(_ skill: RemoteSkill, into folder: URL) async throws {
        let destination = folder.appending(path: skill.name)
        if FileManager.default.fileExists(atPath: destination.path) { throw Failure.taken(skill.name) }
        let staging = try await stage(skill)
        defer { try? FileManager.default.removeItem(at: staging) }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try FileManager.default.moveItem(at: staging, to: destination)
    }

    /// A skill's files, with the note of where they came from, in a temporary folder of their
    /// own (the caller's to remove), ready to go where it's installed.
    @concurrent static func stage(_ skill: RemoteSkill) async throws -> URL {
        let staging = FileManager.default.temporaryDirectory.appending(path: "skill-\(UUID().uuidString)")
        do {
            for file in skill.files {
                let (data, response) = try await URLSession.shared.data(from: rawURL(skill.repository, file))
                guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw Failure.notFound(file) }
                let target = staging.appending(path: skill.relative(file))
                try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
                try data.write(to: target)
            }
            let source = ["repository": skill.repository.id, "path": skill.path, "branch": skill.repository.branch]
            try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
            try JSONSerialization.data(withJSONObject: source, options: [.prettyPrinted, .sortedKeys]).write(to: staging.appending(path: sourceFile))
            return staging
        } catch {
            try? FileManager.default.removeItem(at: staging)
            throw error
        }
    }

    /// Skills installed from GitHub, by their folder's name, with where they came from.
    static func installed(in folder: URL) -> [String: Source] {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: folder.path)) ?? []
        var installed: [String: Source] = [:]
        for name in names {
            guard let data = try? Data(contentsOf: folder.appending(path: name).appending(path: sourceFile)),
                  let source = try? JSONSerialization.jsonObject(with: data) as? [String: String],
                  let repository = source["repository"] else { continue }
            installed[name] = Source(repository: repository, path: source["path"] ?? "")
        }
        return installed
    }

    /// Where a skill came from, kept in its folder.
    static let sourceFile = ".kuvo-source.json"

    private static func rawURL(_ repository: Repository, _ path: String) -> URL {
        let escaped = path.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? path
        return URL(string: "https://raw.githubusercontent.com/\(repository.owner)/\(repository.name)/\(repository.branch)/\(escaped)")!
    }
}
