import Foundation

/// A folder's changed files as its box lists them (`GET kuvo/git/files`, PROTOCOL.md, "Changes"):
/// every decision is the box's (which group a file is in, the scope its actions take, what its
/// stage button does, what its discard comes to and whether that can be undone, how many files a
/// commit takes). A client sends back what the box listed, and words what it says.
public struct GitFiles: Decodable, Equatable, Sendable {
    /// False outside a git repository, where the rest is empty.
    public var repository: Bool
    /// The repository's top folder, which the files' paths start from.
    public var root: String?
    /// The branch checked out (nil on none).
    public var branch: String?
    /// In the order they show: conflicts, staged, unstaged, new.
    public var files: [ListedFile]
    /// Whether anything isn't committed yet.
    public var uncommitted: Bool
    /// What a commit takes now: `staged` (what's staged anywhere in the repository), or `all` of
    /// the folder's changes.
    public var commits: String
    /// How many staged files outside the folder a commit takes along.
    public var stagedElsewhere: Int
    /// How many files a commit takes now, in all, as the box counts them.
    public var takes: Int
    /// How many new files past the most the box lists are left out of `files`.
    public var more: Int
    /// What git is in the middle of: `merging`, `rebasing`, `cherry-picking`, `reverting`.
    public var underway: String?

    private enum CodingKeys: String, CodingKey { case repository, root, branch, files, uncommitted, commits, stagedElsewhere, takes, more, underway }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        repository = try values.decode(Bool.self, forKey: .repository)
        root = try values.decodeIfPresent(String.self, forKey: .root)
        branch = try values.decodeIfPresent(String.self, forKey: .branch)
        files = try values.decodeIfPresent([ListedFile].self, forKey: .files) ?? []
        uncommitted = try values.decodeIfPresent(Bool.self, forKey: .uncommitted) ?? false
        commits = try values.decodeIfPresent(String.self, forKey: .commits) ?? ""
        stagedElsewhere = try values.decodeIfPresent(Int.self, forKey: .stagedElsewhere) ?? 0
        takes = try values.decodeIfPresent(Int.self, forKey: .takes) ?? 0
        more = try values.decodeIfPresent(Int.self, forKey: .more) ?? 0
        underway = try values.decodeIfPresent(String.self, forKey: .underway)
    }

    /// What a commit takes, in words, from what the box says: what it takes (`commits`), how many
    /// files that is (`takes`) and how many of them are staged outside this folder
    /// (`stagedElsewhere`). Nothing is counted here.
    public var commitTakes: String {
        guard commits == "staged" else {
            return takes == 1 ? "Commits the one changed file." : "Commits all \(takes) changed files."
        }
        // (`stagedElsewhere` counts a move's two sides, so it can pass `takes`.)
        if stagedElsewhere == 0 { return takes == 1 ? "Commits the staged file." : "Commits the \(takes) staged files." }
        if takes == 1 { return "Commits the file staged outside this folder." }
        if stagedElsewhere >= takes { return "Commits the \(takes) files staged outside this folder." }
        return "Commits the \(takes) staged files, \(stagedElsewhere) of them outside this folder."
    }
}

/// One changed file as its box lists it: sent back as it is to stage, unstage or discard it, or to
/// read its diff.
public struct ListedFile: Decodable, Hashable, Sendable, Identifiable {
    /// What a file's stage button does, as the box says (`stage`).
    public enum Stage: String, Sendable {
        /// Stages it (`kuvo/git/stage`).
        case stage
        /// Takes it out of what's staged (`kuvo/git/unstage`).
        case unstage
        /// Marks a conflict resolved (`kuvo/git/stage`).
        case resolve
    }

    /// What a file's discard comes to (`discards`), as the box says it.
    public struct Discards: Decodable, Hashable, Sendable {
        /// `trash` (a new file goes to the Trash), `restore` (a deleted file comes back),
        /// `undoMove` (a staged move's old path comes back, the new one goes to the Trash) or
        /// `changes` (its changes are thrown away); a word a newer box says is worded plainly.
        public var kind: String
        /// Nothing is lost for good: what goes, goes to the Trash, or a file comes back.
        public var undoable: Bool

        public init(kind: String, undoable: Bool) {
            self.kind = kind
            self.undoable = undoable
        }
    }

    /// Its path in the repository; a repository of its own inside this one ends in "/".
    public var path: String
    /// Where a staged move came from.
    public var oldPath: String?
    /// The group it shows in: `conflicted`, `staged`, `unstaged`, `untracked`.
    public var group: String
    /// What changed (`added`, `deleted`, `modified`, `renamed`), as its actions take it.
    public var status: String
    /// The scope its actions and its diff take.
    public var scope: String
    /// git's count of its lines added and removed (none for a binary, new or conflicted file).
    public var additions: Int?
    public var deletions: Int?
    /// Where its discard goes back to (`trash`, `unstaged`, `head`); nil for one that isn't
    /// discarded (a conflict, which is resolved).
    public var discard: String?
    /// What it was as listed, which its discard sends back: the box refuses once it changed since.
    public var stamp: String?
    /// What its stage button does; nil for a word this client doesn't know (a newer box's), which
    /// gets no button.
    public var stage: Stage?
    /// What its discard comes to, for the words of its confirm.
    public var discards: Discards?
    /// A repository of its own inside this one, whose commits go with it.
    public var repository: Bool

    /// A file partly staged is listed twice, once in each group.
    public var id: String { group + ":" + path }

    private enum CodingKeys: String, CodingKey { case path, oldPath, group, status, scope, additions, deletions, discard, stamp, stage, discards, repository }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        path = try values.decode(String.self, forKey: .path)
        oldPath = try values.decodeIfPresent(String.self, forKey: .oldPath)
        group = try values.decode(String.self, forKey: .group)
        status = try values.decode(String.self, forKey: .status)
        scope = try values.decode(String.self, forKey: .scope)
        additions = try values.decodeIfPresent(Int.self, forKey: .additions)
        deletions = try values.decodeIfPresent(Int.self, forKey: .deletions)
        discard = try values.decodeIfPresent(String.self, forKey: .discard)
        stamp = try values.decodeIfPresent(String.self, forKey: .stamp)
        stage = try values.decodeIfPresent(String.self, forKey: .stage).flatMap(Stage.init(rawValue:))
        discards = try values.decodeIfPresent(Discards.self, forKey: .discards)
        repository = try values.decodeIfPresent(Bool.self, forKey: .repository) ?? false
    }

    /// Its name, without the folders it's in.
    public var name: String {
        let path = path.hasSuffix("/") ? String(path.dropLast()) : path
        return path.split(separator: "/").last.map(String.init) ?? path
    }

    /// A discard's confirm, in words.
    public struct DiscardWords: Equatable, Sendable {
        public var title: String
        public var detail: String
        /// The words on the button that does it.
        public var button: String
        /// Nothing is lost for good, as the box says: its button isn't drawn as destructive.
        public var undoable: Bool
    }

    /// The words of its discard's confirm, by what the box says the discard comes to (`discards`)
    /// and where it goes back to (`discard`), the same in every client.
    public var discardWords: DiscardWords {
        let toCommit = discard == "head"
        let (title, detail, button): (String, String, String)
        switch discards?.kind {
        case "trash" where repository:
            (title, detail, button) = ("Move the repository \(name) to the Trash?", "It’s a repository of its own: its commits go with it, pushed or not.", "Move to Trash")
        case "trash":
            (title, detail, button) = ("Move \(name) to the Trash?", "It’s a new file, so git keeps nothing of it.", "Move to Trash")
        case "restore" where toCommit:
            (title, detail, button) = ("Restore \(name)?", "The last commit’s copy comes back; a copy on disk that differs goes to the Trash first.", "Restore")
        case "restore":
            (title, detail, button) = ("Restore \(name)?", "It comes back as it was last staged.", "Restore")
        case "undoMove":
            let old = oldPath ?? "its old path"
            (title, detail, button) = ("Undo the move to \(name)?", "\(old) comes back as last committed, and \(name) goes to the Trash. A file written at \(old) since goes to the Trash first.", "Undo Move")
        case "changes" where toCommit:
            (title, detail, button) = ("Discard every change to \(name)?", "It goes back to the last commit, what’s staged too. This can’t be undone.", "Discard")
        case "changes":
            (title, detail, button) = ("Discard the changes to \(name)?", "What isn’t staged is thrown away. This can’t be undone.", "Discard")
        default:
            (title, detail, button) = ("Discard the changes to \(name)?", "", "Discard")
        }
        return DiscardWords(title: title, detail: detail, button: button, undoable: discards?.undoable ?? false)
    }
}

/// One file's diff as its box sends it (`kuvo/git/changes` with `path`): git's patch with the whole
/// file as context; `big` when it's too big to show, `binary` for a binary file's, which has no
/// lines. The box says both: a client never looks through a file's bytes or git's words for them.
public struct GitFileDiff: Decodable, Equatable, Sendable {
    public var patch: String
    public var big: Bool
    public var binary: Bool

    private enum CodingKeys: String, CodingKey { case patch, big, binary }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        patch = try values.decodeIfPresent(String.self, forKey: .patch) ?? ""
        big = try values.decodeIfPresent(Bool.self, forKey: .big) ?? false
        binary = try values.decodeIfPresent(Bool.self, forKey: .binary) ?? false
    }
}
