import ClientChanges
import Foundation
import Testing

/// A folder's changed files as its box lists them (`kuvo/git/files`): what a client words from
/// what the box says, and never works out itself.
struct GitFilesTests {
    private func listed(_ json: String) throws -> GitFiles {
        try JSONDecoder().decode(GitFiles.self, from: Data(json.utf8))
    }

    private func file(_ more: String) throws -> ListedFile {
        try JSONDecoder().decode(ListedFile.self, from: Data(#"{"path":"app/Form.tsx","group":"unstaged","scope":"unstaged",\#(more)}"#.utf8))
    }

    // What a commit takes is the box's count (`takes`), with what it takes (`commits`) and how many
    // of them are staged outside the folder: no row is counted here.
    @Test func whatACommitTakesIsWordedFromTheBoxsCount() throws {
        #expect(try listed(#"{"repository":true,"commits":"staged","stagedElsewhere":1,"takes":1,"files":[]}"#).commitTakes == "Commits the file staged outside this folder.")
        #expect(try listed(#"{"repository":true,"commits":"staged","stagedElsewhere":2,"takes":3,"files":[]}"#).commitTakes == "Commits the 3 staged files, 2 of them outside this folder.")
        #expect(try listed(#"{"repository":true,"commits":"staged","stagedElsewhere":0,"takes":1,"files":[]}"#).commitTakes == "Commits the staged file.")
        #expect(try listed(#"{"repository":true,"commits":"staged","stagedElsewhere":0,"takes":7,"files":[]}"#).commitTakes == "Commits the 7 staged files.")
        // New files past the most it lists are in its count, though not in its rows.
        #expect(try listed(#"{"repository":true,"commits":"all","stagedElsewhere":0,"more":4,"takes":5,"files":[]}"#).commitTakes == "Commits all 5 changed files.")
        #expect(try listed(#"{"repository":true,"commits":"all","stagedElsewhere":0,"takes":1,"files":[]}"#).commitTakes == "Commits the one changed file.")
        // (A move staged outside counts its two sides there, one file here.)
        #expect(try listed(#"{"repository":true,"commits":"staged","stagedElsewhere":4,"takes":2,"files":[]}"#).commitTakes == "Commits the 2 files staged outside this folder.")
    }

    // A discard's confirm says what the box says it comes to (`discards`) and where it goes back
    // to (`discard`); only one the box says loses something for good is destructive.
    @Test func aDiscardsConfirmIsWordedByWhatTheBoxSaysItComesTo() throws {
        let restored = try file(#""status":"deleted","discard":"head","discards":{"kind":"restore","undoable":true}"#).discardWords
        #expect(restored.title == "Restore Form.tsx?" && restored.button == "Restore" && restored.undoable)
        #expect(restored.detail == "The last commit’s copy comes back; a copy on disk that differs goes to the Trash first.")
        #expect(try file(#""status":"deleted","discard":"unstaged","discards":{"kind":"restore","undoable":true}"#).discardWords.detail == "It comes back as it was last staged.")
        let moved = try file(#""status":"renamed","oldPath":"app/Old.tsx","discard":"head","discards":{"kind":"undoMove","undoable":true}"#).discardWords
        #expect(moved.title == "Undo the move to Form.tsx?" && moved.button == "Undo Move" && moved.undoable)
        #expect(moved.detail == "app/Old.tsx comes back as last committed, and Form.tsx goes to the Trash. A file written at app/Old.tsx since goes to the Trash first.")
        let trashed = try file(#""status":"added","discard":"trash","discards":{"kind":"trash","undoable":true}"#).discardWords
        #expect(trashed.title == "Move Form.tsx to the Trash?" && trashed.button == "Move to Trash" && trashed.undoable)
        let thrown = try file(#""status":"modified","discard":"head","discards":{"kind":"changes","undoable":false}"#).discardWords
        #expect(thrown.title == "Discard every change to Form.tsx?" && thrown.button == "Discard" && !thrown.undoable)
        let unstaged = try file(#""status":"modified","discard":"unstaged","discards":{"kind":"changes","undoable":false}"#).discardWords
        #expect(unstaged.title == "Discard the changes to Form.tsx?" && !unstaged.undoable)
        // A new file staged, then changed again: to the Trash in name, but the box says what's lost
        // for good, in its own sentence, and it's drawn as destructive.
        let lost = try file(#""status":"added","discard":"trash","discards":{"kind":"trash","undoable":false,"loses":"What you staged of it isn’t kept anywhere, so this can’t be undone."}"#).discardWords
        #expect(!lost.undoable)
        #expect(lost.detail == "It’s a new file, so git keeps nothing of it. What you staged of it isn’t kept anywhere, so this can’t be undone.")
    }

    // A repository of its own inside this one ("web/") is named without its "/", and its confirm
    // says its commits go with it.
    @Test func aRepositoryInsideIsNamedAndItsCommitsSaidToGo() throws {
        let web = try JSONDecoder().decode(ListedFile.self, from: Data(#"{"path":"apps/web/","group":"untracked","scope":"unstaged","status":"added","stage":"stage","discard":"trash","repository":true,"discards":{"kind":"trash","undoable":true},"stamp":"-:1-2"}"#.utf8))
        #expect(web.name == "web" && web.repository && web.id == "untracked:apps/web/")
        #expect(web.discardWords.title == "Move the repository web to the Trash?")
        #expect(web.discardWords.detail == "It’s a repository of its own: its commits go with it, pushed or not.")
    }

    // What a newer box says that this client doesn't know draws no button, or plain words: never a
    // guess at what it does.
    @Test func wordsANewerBoxSaysAreNeverGuessedAt() throws {
        let unknown = try file(#""status":"modified","stage":"shelve","discard":"head","discards":{"kind":"shelve","undoable":true}"#)
        #expect(unknown.stage == nil)
        #expect(unknown.discardWords.title == "Discard the changes to Form.tsx?" && unknown.discardWords.detail.isEmpty && unknown.discardWords.button == "Discard")
        // A conflict: resolved, not discarded.
        let conflict = try JSONDecoder().decode(ListedFile.self, from: Data(#"{"path":"a.ts","group":"conflicted","scope":"uncommitted","status":"modified","stage":"resolve"}"#.utf8))
        #expect(conflict.stage == .resolve && conflict.discard == nil && conflict.stamp == nil)
    }

    // One file's diff: binary and too big are the box's words.
    @Test func aFilesDiffSaysBinaryAndTooBigAsTheBoxDoes() throws {
        let binary = try JSONDecoder().decode(GitFileDiff.self, from: Data(#"{"repository":true,"patch":"diff --git a/x b/x\nBinary files a/x and b/x differ\n","binary":true}"#.utf8))
        #expect(binary.binary && !binary.big)
        let big = try JSONDecoder().decode(GitFileDiff.self, from: Data(#"{"repository":true,"patch":"","big":true}"#.utf8))
        #expect(big.big && !big.binary && big.patch.isEmpty)
    }
}
