import Foundation
import OpenCodeKit

/// The engine's settings file, `opencode.json` in its own config folder: what it reads as its
/// global configuration, and reloads when the file changes. Settings edit it in place and keep
/// whatever they don't show, so a hand-edited file loses nothing.
@Observable
final class EngineConfig {
    let file: URL
    /// The file's settings; empty when there's no file yet.
    private(set) var settings: [String: JSONValue] = [:]
    /// Why the file couldn't be read. It's left alone until it's fixed.
    private(set) var problem: String?

    /// Settings kept only here, never written anywhere: something being made (a new agent),
    /// filled in by the same controls, until it's created.
    let isDraft: Bool

    init(file: URL) {
        self.file = file
        isDraft = false
        reload()
        Self.current = self
    }

    init(draft settings: [String: JSONValue]) {
        file = URL(filePath: "/dev/null")
        isDraft = true
        self.settings = settings
    }

    /// This Mac's, for the places that make a session's rules.
    static weak var current: EngineConfig?

    /// Saved with a change: machines take it too (`AppModel.settingsSaved`).
    @ObservationIgnored var onSaved: () -> Void = {}
    /// Settings › Permissions' rules changed, wherever from (Settings, a phone, the file edited
    /// by hand): open sessions follow (`Workspace.syncPermissions`).
    @ObservationIgnored var onRulesChanged: () -> Void = {}
    /// The rules as they were last, to tell when they change.
    @ObservationIgnored private var rules: JSONValue?

    private func noteRules() {
        guard settings["permissions"] != rules else { return }
        rules = settings["permissions"]
        onRulesChanged()
    }

    /// Reads the file again and adds what Kuvo Code starts every engine with, unless it sets it:
    /// web search through Exa, which needs no key. The engine has none chosen, and every search
    /// the agent tried failed ("Web search provider is required"). Not "random": that keeps one
    /// service a session, and one that needs a key (Firecrawl) answers every search with nothing.
    ///
    /// It goes in the file, so a choice made anywhere (here, in Settings, in a project's own
    /// settings) wins over it, and only once: what you change or remove later stays as you left
    /// it. A file Kuvo Code can't read is left alone; a new one (none yet, or deleted to start
    /// over) gets it again.
    func addDefaults() {
        reload()
        guard problem == nil else { return }
        let isNew = !FileManager.default.fileExists(atPath: file.path)
        let defaults = UserDefaults.standard
        if isNew || !defaults.bool(forKey: Self.defaultsAddedKey) {
            if settings["websearch"] == nil { add(Self.search, as: "websearch") }
            if problem == nil {
                defaults.set(true, forKey: Self.defaultsAddedKey)
                defaults.set(true, forKey: Self.searchMovedKey)
            }
        } else if !defaults.bool(forKey: Self.searchMovedKey) {
            // "random", as Kuvo Code chose it before: Exa instead, once.
            if value(at: ["websearch"]) == .object(["provider": .string("random")]) { set(Self.search, at: ["websearch"]) }
            if problem == nil { defaults.set(true, forKey: Self.searchMovedKey) }
        }
    }

    private static let search: JSONValue = .object(["provider": .string("exa")])
    private static let defaultsAddedKey = "engine.defaultsAdded"
    private static let searchMovedKey = "engine.searchMovedToExa"

    /// Adds a setting to the file as it's written, keeping its layout and order: one line at its
    /// end. A file that's new, or not laid out as expected, is written whole, as `set` does.
    private func add(_ value: JSONValue, as key: String) {
        if let original = try? String(contentsOf: file, encoding: .utf8),
           let text = Self.adding(key, value, to: original),
           let parsed = try? JSONDecoder().decode([String: JSONValue].self, from: Data(text.utf8)),
           parsed == settings.merging([key: value], uniquingKeysWith: { $1 }) {
            do {
                try Data(text.utf8).write(to: target, options: .atomic)
                settings = parsed
                noteRules()
            } catch {
                problem = "It couldn't be saved: \(error.localizedDescription)"
            }
        } else {
            set(value, at: [key])
        }
    }

    /// `text`, a JSON object, with `key: value` added as its last member, laid out like the rest
    /// (on a line of its own, indented as the one before it, or all on one line); nil when it
    /// doesn't end as an object does.
    static func adding(_ key: String, _ value: JSONValue, to text: String) -> String? {
        guard let close = text.lastIndex(where: { !$0.isWhitespace }), text[close] == "}",
              let last = text[..<close].lastIndex(where: { !$0.isWhitespace }),
              let member = inline(.object([key: value])).map({ String($0.dropFirst().dropLast()) }) else { return nil }
        let isEmpty = text[last] == "{"
        let head = String(text[...last]), tail = String(text[close...])
        // The file's own line ending ("\r\n" is one Character to Swift, so it's read by scalar).
        let scalars = text.unicodeScalars
        let newline = scalars.contains("\r") ? (scalars.contains("\n") ? "\r\n" : "\r") : scalars.contains("\n") ? "\n" : nil
        guard let newline else { return head + (isEmpty ? "" : ", ") + member + tail }
        let lineStart = text[..<last].lastIndex(where: \.isNewline).map { text.index(after: $0) } ?? text.startIndex
        let indent = isEmpty ? "  " : String(text[lineStart...].prefix { $0 == " " || $0 == "\t" })
        return head + (isEmpty ? "" : ",") + newline + indent + member + newline + tail
    }

    /// `value` as JSON on one line, spaced as people write it: `{"provider": "random"}`.
    private static func inline(_ value: JSONValue) -> String? {
        switch value {
        case .object(let object):
            var members: [String] = []
            for key in object.keys.sorted() {
                guard let name = inline(.string(key)), let value = inline(object[key]!) else { return nil }
                members.append(name + ": " + value)
            }
            return "{" + members.joined(separator: ", ") + "}"
        case .array(let items):
            var texts: [String] = []
            for item in items {
                guard let text = inline(item) else { return nil }
                texts.append(text)
            }
            return "[" + texts.joined(separator: ", ") + "]"
        default:
            let encoder = JSONEncoder()
            encoder.outputFormatting = .withoutEscapingSlashes
            return (try? encoder.encode(value)).map { String(decoding: $0, as: UTF8.self) }
        }
    }

    /// Where the file is written: through a link (to a dotfiles folder, say) to the file it names,
    /// so the link stays a link.
    private var target: URL {
        var url = file.resolvingSymlinksInPath()
        // A link to a file that isn't there yet, which that leaves as it is.
        for _ in 0..<8 {
            guard let destination = try? FileManager.default.destinationOfSymbolicLink(atPath: url.path) else { break }
            url = URL(filePath: destination, relativeTo: url.deletingLastPathComponent()).standardizedFileURL
        }
        return url
    }

    /// Settings › Permissions' rules, in their order: what every session and mode is held to.
    var permissionRules: [PermissionRule] {
        guard let value = settings["permissions"], let data = try? JSONEncoder().encode(value) else { return [] }
        return (try? JSONDecoder().decode([PermissionRule].self, from: data)) ?? []
    }

    /// Reads the file again, after it was changed elsewhere.
    func reload() {
        guard !isDraft else { return }
        guard let data = try? Data(contentsOf: file) else {
            settings = [:]
            problem = nil
            noteRules()
            return
        }
        do {
            settings = data.isEmpty ? [:] : try JSONDecoder().decode([String: JSONValue].self, from: data)
            problem = nil
            noteRules()
        } catch {
            problem = "It isn't plain JSON, so Kuvo Code leaves it alone. (Comments aren't supported here.)"
        }
    }

    /// The value at a path of keys: `value(at: ["compaction", "auto"])`.
    func value(at path: [String]) -> JSONValue? {
        var current: JSONValue? = .object(settings)
        for key in path { current = current?[key] }
        return current
    }

    /// Sets the value at a path of keys, or removes it with nil (and any object it leaves
    /// empty), then saves.
    func set(_ value: JSONValue?, at path: [String]) {
        guard problem == nil, !path.isEmpty else { return }
        settings = Self.setting(value, at: path[...], in: settings)
        save()
    }

    /// Several changes at once, in one write: the engine reloads its settings once, not after
    /// each of them.
    func set(_ changes: [(path: [String], value: JSONValue?)]) {
        guard problem == nil else { return }
        for change in changes where !change.path.isEmpty {
            settings = Self.setting(change.value, at: change.path[...], in: settings)
        }
        save()
    }

    private static func setting(_ value: JSONValue?, at path: ArraySlice<String>, in object: [String: JSONValue]) -> [String: JSONValue] {
        var object = object
        let key = path.first!
        if path.count == 1 {
            object[key] = value
        } else {
            let inner = setting(value, at: path.dropFirst(), in: object[key]?.object ?? [:])
            object[key] = inner.isEmpty ? nil : .object(inner)
        }
        return object
    }

    private func save() {
        guard !isDraft else { return }
        // No "$schema": opencode's published schema is still v1's, and would mark v2's keys
        // as errors in an editor.
        let document = settings
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        do {
            let target = target
            try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
            try encoder.encode(document).write(to: target, options: .atomic)
            settings = document
            onSaved()
            noteRules()
        } catch {
            problem = "It couldn't be saved: \(error.localizedDescription)"
        }
    }
}

// MARK: - Reading values

extension JSONValue {
    /// Whole numbers as `Int`: nil for one that isn't whole or doesn't fit.
    var int: Int? { number.flatMap { Int(exactly: $0) } }
}
