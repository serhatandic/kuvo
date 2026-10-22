import OpenCodeKit
import SwiftUI

/// Skills: instructions and scripts the agent loads when a task calls for them. The ones it
/// has, each can be turned off; popular collections on GitHub to add more from; other folders
/// and catalogs to read them from.
struct SkillsSettings: View {
    @Environment(AppModel.self) private var app
    @Environment(\.settingsBox) private var box
    /// Nil until the engine has listed them.
    @State private var skills: [Skill]?
    @State private var installed: [String: SkillLibrary.Source] = [:]
    @State private var browsing: SkillLibrary.Repository?
    @State private var repositoryName = ""
    /// While a repository typed by name is being looked up on GitHub.
    @State private var isOpening = false
    @State private var failure: String?
    /// The skill being written or changed: a new one has no name yet.
    @State private var writing: SkillDraft?
    /// What the list is narrowed to.
    @State private var search = ""

    private var config: EngineConfig { app.config(for: box) }
    private var folder: URL { app.skillsFolder }

    /// The skills, narrowed to what's searched for.
    private var shown: [Skill] {
        let query = search.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return skills ?? [] }
        return (skills ?? []).filter { "\($0.name) \($0.description ?? "")".localizedCaseInsensitiveContains(query) }
    }

    /// A description's first sentence.
    static func summary(_ description: String) -> String {
        let text = description.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let end = text.range(of: #"[.!?](\s|$)"#, options: .regularExpression) else { return text }
        return String(text[..<end.upperBound]).trimmingCharacters(in: .whitespaces)
    }

    var body: some View {
        ScrollingSettingsForm {
            Section {
                if skills == nil {
                    Spinner(size: 16)
                        .frame(maxWidth: .infinity)
                } else if skills?.isEmpty == true {
                    Text("None yet.")
                        .foregroundStyle(.secondary)
                } else if (skills?.count ?? 0) > 6 {
                    SettingsSearchRow(text: $search, prompt: "Search skills")
                }
                ForEach(shown) { skill in
                    SettingsRowLayout {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("\(skill.name)  \(Text(source(of: skill) ?? "").font(.subheadline).foregroundStyle(.secondary))")
                            if let description = skill.description {
                                // Written for the model, often at length: its first sentence,
                                // and all of it on hover.
                                Text(Self.summary(description))
                                    .font(.callout)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(2)
                                    .help(description)
                            }
                        }
                        .settingsText()
                        HStack {
                            if isYours(skill) {
                                // Deleting one you wrote is in its editor, behind a confirmation.
                                Button("Edit…") { writing = SkillDraft(name: skill.id) }
                            }
                            if installed[skill.id] != nil {
                                // From GitHub, so it can be added back.
                                Button("Remove") { remove(skill.id) }
                            }
                            Toggle("Use \(skill.name)", isOn: isOn(skill.id))
                                .labelsHidden()
                                .toggleStyle(.switch)
                                .controlSize(.small)
                        }
                    }
                    // Where it is, for one found in a folder of its own.
                    .help(place(of: skill) ?? "")
                }
                Button("New Skill…") { writing = SkillDraft(name: nil) }
            } header: {
                Text("Skills the Agent Can Use")
            } footer: {
                Text("The agent loads a skill when a task calls for it. Skills in ~/.claude/skills and ~/.agents/skills, and in a project's .opencode/skills, are found on their own.")
                    .foregroundStyle(.secondary)
            }

            ConfigProblem()

            Section {
                ForEach(SkillLibrary.repositories) { repository in
                    Button {
                        browsing = repository
                    } label: {
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                HStack(spacing: 6) {
                                    Text(repository.title)
                                    Text(repository.id)
                                        .font(.subheadline)
                                        .foregroundStyle(.secondary)
                                }
                                Text(repository.summary)
                                    .font(.callout)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            Image(systemName: "chevron.right")
                                .foregroundStyle(.secondary)
                        }
                    }
                    .buttonStyle(TextButtonStyle(outset: true, dimsAtRest: false))
                }
                HStack {
                    TextField("Repository", text: $repositoryName, prompt: Text("Another repository on GitHub, like owner/skills"))
                        .labelsHidden()
                        .onSubmit(openRepository)
                    if isOpening {
                        Spinner(size: 16)
                    }
                    Button("Open", action: openRepository)
                        .disabled(isOpening || repositoryName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                if let failure {
                    Text(failure).foregroundStyle(.red)
                }
            } header: {
                Text("Get Skills")
            } footer: {
                Text("These come from their authors on GitHub, not from Kuvo Code. A skill can include scripts the agent may run, so look at what's in one before you add it.")
                    .foregroundStyle(.secondary)
            }

            Section {
                ConfigStringList(path: ["skills"], prompt: "A folder, or the address of a skill catalog")
            } header: {
                Text("Other Places")
            } footer: {
                Text("A catalog is a web address with an index.json listing its skills.")
                    .foregroundStyle(.secondary)
            }
        }
        .task(id: config.settings) { await refresh() }
        .settingsDetail(item: $writing, title: { $0.name ?? "New Skill" }) { draft in
            SkillEditor(name: draft.name, folder: folder, taken: Set((skills ?? []).map(\.id))) {
                writing = nil
                Task { await refresh() }
            }
        }
        .settingsDetail(item: $browsing, title: { $0.title }) { repository in
            RepositorySkills(repository: repository, folder: folder) {
                Task { await refresh() }
            } done: {
                browsing = nil
            }
        }
    }

    private func refresh() async {
        installed = SkillLibrary.installed(in: folder)
        // The engine notices a new folder within a moment.
        try? await Task.sleep(for: .milliseconds(500))
        skills = ((try? await app.api(for: box)?.skills()) ?? []).sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    /// One you wrote here, rather than added from GitHub or found elsewhere.
    private func isYours(_ skill: Skill) -> Bool {
        installed[skill.id] == nil && skill.path?.hasPrefix(folder.path + "/") == true
    }

    /// Where a skill comes from, when that's short to say: built in, added here, or the GitHub
    /// repository it came from. One found in a folder (~/.claude/skills) goes without.
    private func source(of skill: Skill) -> String? {
        if let source = installed[skill.id] { return source.repository }
        guard let path = skill.path else { return nil }
        if path.hasPrefix("/builtin/") { return "Built in" }
        if path.hasPrefix(folder.path + "/") { return "Added here" }
        return nil
    }

    /// The folder a skill found elsewhere is in, for its row's tip.
    private func place(of skill: Skill) -> String? {
        guard source(of: skill) == nil, let path = skill.path else { return nil }
        return (path as NSString).abbreviatingWithTildeInPath
            .components(separatedBy: "/").dropLast(2).joined(separator: "/")
    }

    /// Off is a rule that denies the skill; turning it on takes that rule out, and leaves any
    /// other rule for it alone.
    private func isOn(_ id: String) -> Binding<Bool> {
        Binding {
            !config.items(["permissions"]).compactMap(\.rule).contains { $0.action == "skill" && $0.resource == id && $0.effect == .deny }
        } set: { on in
            var items = config.items(["permissions"]).filter { item in
                guard let rule = item.rule else { return true }
                return !(rule.action == "skill" && rule.resource == id && rule.effect == .deny)
            }
            if !on { items.append(PermissionRule(action: "skill", resource: id, effect: .deny).json) }
            config.setItems(items, at: ["permissions"])
        }
    }

    private func remove(_ id: String) {
        try? FileManager.default.moveToTrash(folder.appending(path: id))
        Task { await refresh() }
    }

    private func openRepository() {
        let name = repositoryName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, !isOpening else { return }
        failure = nil
        isOpening = true
        Task {
            defer { isOpening = false }
            do {
                let repository = try await SkillLibrary.repository(named: name)
                // A skill you started writing, or a repository you opened, meanwhile stays in view,
                // and the name stays typed.
                guard writing == nil, browsing == nil else { return }
                browsing = repository
                repositoryName = ""
            } catch {
                failure = error.localizedDescription
            }
        }
    }
}

private struct SkillDraft: Identifiable {
    /// Nil for a new skill.
    var name: String?
    var id: String { name ?? "" }
}

/// Writing a skill of your own, in the page's place: its name, when the agent should use it, and
/// its instructions, in a SKILL.md in its own folder (where scripts and other files it needs can
/// go too).
private struct SkillEditor: View {
    /// Nil for a new skill.
    let name: String?
    let folder: URL
    /// Names other skills have.
    let taken: Set<String>
    let done: () -> Void

    @State private var newName = ""
    @State private var summary = ""
    @State private var instructions = ""
    @State private var failure: String?
    @State private var confirmsRemoval = false

    private var id: String { name ?? SkillFile.name(from: newName) }
    private var file: URL { folder.appending(path: id).appending(path: "SKILL.md") }
    private var canSave: Bool {
        !id.isEmpty && !summary.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && (name != nil || !isTaken)
    }

    /// A new skill's name is one there is already: in the engine's list, or as a folder here
    /// (while the list loads, or when it couldn't be read), which saving would write over.
    private var isTaken: Bool {
        taken.contains(id) || FileManager.default.fileExists(atPath: folder.appending(path: id).path)
    }

    /// What's under a new skill's name: the name it's saved as when that isn't what you typed,
    /// or why it can't be saved. Always a line, so the fields below don't move as you type.
    private var nameNote: (text: String, isProblem: Bool) {
        let typed = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        if !typed.isEmpty, id.isEmpty { return ("Names use the letters a to z and digits, and this one has none.", true) }
        if !id.isEmpty, isTaken { return ("There's a skill called \(id) already.", true) }
        if !typed.isEmpty, id != typed { return ("It's saved as \(id).", false) }
        return ("Names use lowercase letters a to z, digits and hyphens.", false)
    }

    var body: some View {
        SettingsDetailPage {
            if name == nil {
                Section {
                    SettingsTextRow(title: "Name") { TextField("Name", text: $newName, prompt: Text("release-notes")) }
                } footer: {
                    Text(nameNote.text)
                        .foregroundStyle(nameNote.isProblem ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                }
            }
            Section {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Use it when")
                    TextField("Use it when", text: $summary, prompt: Text("Writing release notes from the commits since the last tag"), axis: .vertical)
                        .labelsHidden()
                        .lineLimit(1...10)
                }
            } footer: {
                Text("The agent reads this to decide when the skill is needed, and loads the instructions only then.")
                    .foregroundStyle(.secondary)
            }
            Section {
                TextEditor(text: $instructions)
                    .font(.system(.body, design: .monospaced))
                    .frame(minHeight: 260)
            } header: {
                Text("Instructions")
            } footer: {
                Text("Markdown. Scripts and other files can go in the skill's folder; mention them here so the agent knows to use them.")
                    .foregroundStyle(.secondary)
            }
        } footer: {
            VStack(alignment: .leading, spacing: 10) {
                if let failure {
                    Text(failure)
                        .font(.callout)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
                EditorFooter(primary: name == nil ? "Create Skill" : "Save", canDoPrimary: canSave, onPrimary: save, onCancel: done) {
                    if name != nil {
                        EditorFooterRemove(title: "Delete Skill…") { confirmsRemoval = true }
                        Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([file]) }
                            .buttonStyle(.glass)
                    }
                }
            }
        }
        .onAppear(perform: load)
        .confirmationDialog("Delete \(id)?", isPresented: $confirmsRemoval) {
            Button("Delete", role: .destructive, action: delete)
        } message: {
            Text("Its folder, with anything else in it, goes to the Trash.")
        }
    }

    private func load() {
        guard name != nil, let text = try? String(contentsOf: file, encoding: .utf8) else { return }
        let parsed = SkillFile.parse(text)
        summary = parsed.description
        instructions = parsed.body
    }

    private func save() {
        guard name != nil || !isTaken else {
            failure = "There's a skill called \(id) already."
            return
        }
        do {
            try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
            // A skill's own file keeps whatever else its header has.
            let existing = name == nil ? nil : try? String(contentsOf: file, encoding: .utf8)
            try SkillFile.text(name: id, description: summary, body: instructions, replacing: existing).write(to: file, atomically: true, encoding: .utf8)
            if name == nil { Analytics.shared.track("skill_written") }
            done()
        } catch {
            failure = error.localizedDescription
        }
    }

    private func delete() {
        do {
            try FileManager.default.moveToTrash(file.deletingLastPathComponent())
            done()
        } catch {
            failure = error.localizedDescription
        }
    }
}

/// A SKILL.md: a header with its name and description, then its instructions.
nonisolated enum SkillFile {
    /// A skill's name as skills want it: lowercase letters, digits and hyphens.
    /// A skill's name from what's typed: its words in a to z and digits, joined by single
    /// hyphens (words in other scripts left out, so none at the start, the end or doubled).
    static func name(from text: String) -> String {
        let words = text.lowercased().split { !$0.isLetter && !$0.isNumber }.map { $0.filter(\.isASCII) }.filter { !$0.isEmpty }
        var name = String(words.joined(separator: "-").prefix(64))
        while name.hasSuffix("-") { name.removeLast() }
        return name
    }

    static func parse(_ text: String) -> (description: String, body: String) {
        guard text.hasPrefix("---"), let end = text.range(of: "\n---", range: text.index(text.startIndex, offsetBy: 3)..<text.endIndex) else {
            return ("", text)
        }
        let header = text[text.index(text.startIndex, offsetBy: 3)..<end.lowerBound]
        var description = ""
        for line in header.split(separator: "\n") where line.hasPrefix("description:") {
            description = unquote(line.dropFirst("description:".count).trimmingCharacters(in: .whitespaces))
        }
        var body = String(text[end.upperBound...])
        if let newline = body.firstIndex(of: "\n") { body = String(body[body.index(after: newline)...]) }
        return (description, body.trimmingCharacters(in: .newlines))
    }

    /// The file's text. `existing`'s header lines other than the name and description stay.
    static func text(name: String, description: String, body: String, replacing existing: String? = nil) -> String {
        var others: [String] = []
        if let existing, existing.hasPrefix("---"),
           let end = existing.range(of: "\n---", range: existing.index(existing.startIndex, offsetBy: 3)..<existing.endIndex) {
            others = existing[existing.index(existing.startIndex, offsetBy: 3)..<end.lowerBound].split(separator: "\n", omittingEmptySubsequences: false)
                .map(String.init).filter { !$0.isEmpty && !$0.hasPrefix("name:") && !$0.hasPrefix("description:") }
        }
        let summary = description.split(whereSeparator: \.isNewline).joined(separator: " ").trimmingCharacters(in: .whitespaces)
        let header = (["name: \(name)", "description: \(quote(summary))"] + others).joined(separator: "\n")
        return "---\n\(header)\n---\n\n\(body.trimmingCharacters(in: .newlines))\n"
    }

    /// YAML's double quotes, so colons and quotes in it read as text.
    private static func quote(_ text: String) -> String {
        "\"" + text.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
    }

    private static func unquote(_ text: String) -> String {
        if text.count >= 2, text.hasPrefix("\""), text.hasSuffix("\"") {
            return String(text.dropFirst().dropLast()).replacingOccurrences(of: "\\\"", with: "\"").replacingOccurrences(of: "\\\\", with: "\\")
        }
        if text.count >= 2, text.hasPrefix("'"), text.hasSuffix("'") {
            return String(text.dropFirst().dropLast()).replacingOccurrences(of: "''", with: "'")
        }
        return text
    }
}

/// A repository's skills, each with what it says it does, to add or remove.
private struct RepositorySkills: View {
    let repository: SkillLibrary.Repository
    let folder: URL
    let changed: () -> Void
    let done: () -> Void
    @State private var skills: [SkillLibrary.RemoteSkill]?
    @State private var isPartial = false
    @State private var installed: [String: SkillLibrary.Source] = [:]
    @State private var query = ""
    /// Why the list couldn't be loaded.
    @State private var failure: String?
    /// Why a skill couldn't be added: the list stays.
    @State private var installFailure: String?
    @State private var confirming: SkillLibrary.RemoteSkill?
    @State private var working: String?

    /// The filter, without the spaces around it.
    private var search: String { query.trimmingCharacters(in: .whitespaces) }

    private var shown: [SkillLibrary.RemoteSkill] {
        guard let skills else { return [] }
        return search.isEmpty ? skills : skills.filter { $0.path.localizedCaseInsensitiveContains(search) }
    }

    var body: some View {
        SettingsDetailPage {
            Section {
                LabeledContent("On GitHub") {
                    Link(repository.id, destination: repository.webURL)
                }
            }
            Section {
                if let failure {
                    Label {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Couldn’t load the skills")
                            Text(failure)
                                .font(.callout)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    } icon: {
                        Image(systemName: "exclamationmark.triangle")
                    }
                } else if skills == nil {
                    HStack {
                        Spacer()
                        Spinner(size: 20)
                        Spacer()
                    }
                    .padding(.vertical, 16)
                } else {
                    SettingsSearchRow(text: $query, prompt: "Filter skills")
                    if shown.isEmpty {
                        Text(search.isEmpty ? "This repository doesn’t have any skills." : "No skills match “\(search)”.")
                            .foregroundStyle(.secondary)
                    }
                    ForEach(shown) { skill in
                        RemoteSkillRow(skill: skill, installed: installed[skill.name], isWorking: working == skill.id) {
                            confirming = skill
                        } remove: {
                            try? FileManager.default.moveToTrash(folder.appending(path: skill.name))
                            reload()
                        }
                    }
                }
            } header: {
                Text("Skills")
            } footer: {
                if isPartial {
                    Text("This collection is too big for GitHub to list in full, so some of its skills aren't here.")
                        .foregroundStyle(.secondary)
                }
            }
        } footer: {
            EditorFooter(primary: "Done", onPrimary: done)
        }
        .task {
            reload()
            do {
                let listing = try await SkillLibrary.skills(in: repository)
                skills = listing.skills
                isPartial = listing.isPartial
            } catch {
                failure = error.localizedDescription
            }
        }
        .alert("Couldn't Add the Skill", isPresented: Binding(get: { installFailure != nil }, set: { if !$0 { installFailure = nil } })) {
            Button("OK") {}
        } message: {
            Text(installFailure ?? "")
        }
        .confirmationDialog("Add \(confirming?.name ?? "")?", isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }), presenting: confirming) { skill in
            Button("Add Skill") { install(skill) }
            Button("Cancel", role: .cancel) {}
        } message: { skill in
            Text(Self.summary(of: skill))
        }
    }

    /// Its files, saying so when some are scripts.
    static func summary(of skill: SkillLibrary.RemoteSkill) -> String {
        let names = skill.files.map { skill.relative($0) }
        let scripts = names.filter { ["sh", "py", "js", "ts", "rb", "pl", "bash", "zsh"].contains(($0 as NSString).pathExtension.lowercased()) }
        var lines = names.prefix(12).map { "• " + $0 }
        if names.count > 12 { lines.append("and \(names.count - 12) more") }
        let note = scripts.isEmpty ? "" : "\n\nIt has \(scripts.count == 1 ? "a script" : "\(scripts.count) scripts") the agent may run."
        return "From \(skill.repository.id):\n\n" + lines.joined(separator: "\n") + note
    }

    private func reload() {
        installed = SkillLibrary.installed(in: folder)
        changed()
    }

    private func install(_ skill: SkillLibrary.RemoteSkill) {
        working = skill.id
        Task {
            defer { working = nil }
            do {
                try await SkillLibrary.install(skill, into: folder)
                // The collection by name when it's one of the listed ones, else just that it was another.
                let listed = SkillLibrary.repositories.contains(skill.repository)
                Analytics.shared.track("skill_added", ["collection": listed ? "\(skill.repository.owner)/\(skill.repository.name)" : "other"])
                reload()
            } catch {
                installFailure = error.localizedDescription
            }
        }
    }
}

private struct RemoteSkillRow: View {
    let skill: SkillLibrary.RemoteSkill
    /// The skill installed under this one's name, from wherever it came.
    let installed: SkillLibrary.Source?
    let isWorking: Bool
    let add: () -> Void
    let remove: () -> Void
    @State private var about: SkillLibrary.About?

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text(about?.name ?? skill.name)
                Text(about?.description ?? " ")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
            }
            Spacer()
            if isWorking {
                Spinner(size: 16)
            } else if let installed {
                if installed.repository == skill.repository.id, installed.path == skill.path {
                    Button("Remove", action: remove)
                        .accessibilityLabel("Remove \(skill.name)")
                } else {
                    // Another skill by this name is installed (they'd share a folder).
                    Text("Another \(skill.name) is added, from \(installed.repository)")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.trailing)
                }
            } else {
                Button("Add", action: add)
                    .accessibilityLabel("Add \(skill.name)")
            }
        }
        .task { about = await SkillLibrary.about(skill) }
    }
}
