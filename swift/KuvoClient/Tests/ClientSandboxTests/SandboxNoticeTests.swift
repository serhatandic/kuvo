import ClientSandbox
import Foundation
import Testing

/// What Settings › Permissions says of a computer's sandbox, from what its box answers: a quiet line
/// when it's on, and when it isn't, why and the one button that fixes it, the same on both apps.
struct SandboxNoticeTests {
    private func state(_ json: String) throws -> SandboxState {
        try JSONDecoder().decode(SandboxState.self, from: Data(json.utf8))
    }

    // Not on: the row is the box's own sentence (`why`) and its button's words (`actionLabel`),
    // whatever the reason. No client keeps words for a reason.
    @Test func onIsAQuietLineAndAnythingElseSaysTheBoxsWordsWithItsButton() throws {
        let on = try state(#"{"on": true, "able": true, "agents": false}"#).notice
        #expect(on.look == .on && on.title == nil && on.buttons.isEmpty)

        let failed = try state(#"{"on": false, "needsYou": true, "reason": "checkFailed", "check": "readsOwn", "why": "Its check found that a command could still read Kuvo Code's files.", "action": "retry", "actionLabel": "Try Again", "route": "POST kuvo/sandbox/check", "failures": 1}"#).notice
        #expect(failed.look == .needsYou)
        #expect(failed.title == "The sandbox isn’t on")
        #expect(failed.detail == "Its check found that a command could still read Kuvo Code's files.")
        #expect(failed.buttons == [.init(title: "Try Again", action: .check)])
        let again = try state(#"{"on": false, "needsYou": true, "reason": "checkFailed", "check": "writesOwn", "why": "Its check failed.", "action": "retry", "actionLabel": "Try Again", "route": "POST kuvo/sandbox/check", "failures": 3, "report": "The sandbox’s check failed 3 times in a row: Its check failed. (check: writesOwn)"}"#).notice
        #expect(again.buttons.map(\.action) == [.check, .report], "Report a Problem joins where the box says what a report says")
        // (Failures alone, with no report from the box: none.)
        #expect(try state(#"{"on": false, "needsYou": true, "reason": "checkFailed", "why": "Its check failed.", "action": "retry", "actionLabel": "Try Again", "route": "POST kuvo/sandbox/check", "failures": 3}"#).notice.buttons.map(\.action) == [.check])

        // The button's words are the box's, and an ellipsis says a client asks first.
        #expect(try state(#"{"on": false, "needsYou": true, "reason": "plugin", "why": "Its plugin for the engine was changed or removed.", "action": "fix", "actionLabel": "Fix It", "route": "POST kuvo/sandbox/check"}"#).notice.buttons == [.init(title: "Fix It", action: .check)])
        #expect(try state(#"{"on": false, "needsYou": true, "reason": "engineMissing", "why": "The engine isn't installed here yet.", "action": "retry", "actionLabel": "Check Again", "route": "POST kuvo/sandbox/check"}"#).notice.buttons == [.init(title: "Check Again", action: .check)])
        #expect(try state(#"{"on": false, "needsYou": true, "reason": "waiting", "why": "Waiting for the engine to load it.", "action": "restartEngine", "actionLabel": "Restart Engine…", "route": "POST kuvo/engine/restart"}"#).notice.buttons
                == [.init(title: "Restart Engine…", action: .restartEngine(asking: true))])
        #expect(try state(#"{"on": false, "needsYou": true, "reason": "engineDown", "why": "The engine isn't running.", "action": "restartEngine", "actionLabel": "Restart Engine", "route": "POST kuvo/engine/restart"}"#).notice.buttons
                == [.init(title: "Restart Engine", action: .restartEngine(asking: false))])
        // Being checked: a quiet line, in the box's words.
        let checking = try state(#"{"on": false, "needsYou": true, "reason": "checking", "why": "It's being checked.", "action": "retry", "actionLabel": "Try Again", "route": "POST kuvo/sandbox/check"}"#).notice
        #expect(checking.look == .checking && checking.detail == "It's being checked." && checking.buttons.isEmpty)
        // No words for a button: none is drawn.
        #expect(try state(#"{"on": false, "needsYou": true, "reason": "plugin", "why": "Changed.", "action": "fix", "route": "POST kuvo/sandbox/check"}"#).notice.buttons.isEmpty)
    }

    // A button with no route opens what the box names (`opens`): a web page from anywhere; an
    // address that's that computer's own (`opensHere`) is marked so, for only an app on it to draw.
    @Test func aComputerThatCantHaveItSaysWhyAndWhereToLook() throws {
        let linux = try state(#"{"on": false, "needsYou": true, "able": false, "reason": "kernel", "action": "help", "actionLabel": "How to Check…", "opens": "https://www.kuvo.run/code/docs/machines/#sandbox", "needs": "Linux 5.13", "why": "Linux 5.10 can't sandbox commands: it needs Landlock (Linux 5.13 or later, with it on)."}"#).notice
        #expect(linux.detail == "Linux 5.10 can't sandbox commands: it needs Landlock (Linux 5.13 or later, with it on).")
        #expect(linux.buttons == [.init(title: "How to Check…", action: .help, opens: URL(string: "https://www.kuvo.run/code/docs/machines/#sandbox"))])
        let mac = try state(#"{"on": false, "needsYou": true, "able": false, "reason": "macos", "why": "macOS's sandbox isn't on this Mac.", "action": "softwareUpdate", "actionLabel": "Software Update…", "opens": "x-apple.systempreferences:com.apple.Software-Update-Settings.extension", "opensHere": true}"#).notice
        #expect(mac.buttons.map(\.action) == [.softwareUpdate])
        #expect(mac.buttons.first?.opensHere == true)
        // Never anything but a web page at another computer's word, nor a button with nowhere to go.
        #expect(try state(#"{"on": false, "needsYou": true, "reason": "kernel", "why": "Too old.", "action": "help", "actionLabel": "How to Check…", "opens": "file:///etc/passwd"}"#).notice.buttons.isEmpty)
        #expect(try state(#"{"on": false, "needsYou": true, "reason": "kernel", "why": "Too old.", "action": "help", "actionLabel": "How to Check…"}"#).notice.buttons.isEmpty)
    }

    @Test func aMachineWhereItCantKeepCommandsFromYourServicesSaysSo() throws {
        // (A Linux machine whose commands can't have a namespace of their own: on, with a gap.)
        // Its words and its page are the box's (`sandbox.report`).
        let without = try state(#"{"on": true, "needsYou": true, "able": true, "agents": true, "gaps": ["services"], "needs": "user namespaces", "why": "Commands run in the sandbox, but this machine doesn’t let it keep them from your login’s services, like systemd. Until it allows user namespaces, agents don’t use your vault here and goals don’t run on their own.", "action": "help", "actionLabel": "How to Check…", "opens": "https://www.kuvo.run/code/docs/machines/#sandbox"}"#).notice
        #expect(without.look == .needsYou, "not the green line that says all's well")
        #expect(without.title == "Sandbox", "the box's sentence follows, so the title doesn't say it again")
        #expect(without.detail == "Commands run in the sandbox, but this machine doesn’t let it keep them from your login’s services, like systemd. Until it allows user namespaces, agents don’t use your vault here and goals don’t run on their own.")
        #expect(without.buttons == [.init(title: "How to Check…", action: .help, opens: URL(string: "https://www.kuvo.run/code/docs/machines/#sandbox"))])
        // A gap a newer box adds reads as none.
        #expect(try state(#"{"on": true, "able": true, "agents": true, "gaps": ["later"]}"#).notice.detail.contains("can’t read your keys"))
    }

    // #739: whether it warns is the box's one fact (`needsYou`), not a rule of the client's: a gap a
    // newer box names warns with its words where the box says it needs you.
    @Test func itWarnsWhereTheBoxSaysItNeedsYou() throws {
        let newer = try state(#"{"on": true, "needsYou": true, "able": true, "agents": true, "gaps": ["later"], "why": "Commands run in the sandbox, but not all of it is kept here yet.", "action": "help", "actionLabel": "How to Check…", "opens": "https://www.kuvo.run/code/docs/machines/#sandbox"}"#).notice
        #expect(newer.look == .needsYou && newer.title == "Sandbox")
        #expect(newer.detail == "Commands run in the sandbox, but not all of it is kept here yet.")
        #expect(newer.buttons.map(\.action) == [.help])
    }

    @Test func aReasonFromANewerBoxShowsItsOwnWords() throws {
        let newer = try state(#"{"on": false, "needsYou": true, "reason": "somethingNew", "why": "Its new part isn't ready.", "action": "somethingElse", "actionLabel": "Make It Ready", "route": "POST kuvo/sandbox/check"}"#).notice
        #expect(newer.look == .needsYou)
        #expect(newer.detail == "Its new part isn't ready.")
        // An action it doesn't know either: its words, where it sends the check's route.
        #expect(newer.buttons == [.init(title: "Make It Ready", action: .check)])
        let known = try state(#"{"on": false, "needsYou": true, "reason": "somethingNew", "why": "Its new part isn't ready.", "action": "restartEngine", "actionLabel": "Restart Engine…", "route": "POST kuvo/engine/restart"}"#).notice
        #expect(known.buttons.map(\.action) == [.restartEngine(asking: true)])
        let unknown = try state(#"{"on": false, "needsYou": true, "reason": "somethingNew", "why": "Its new part isn't ready.", "action": "somethingElse", "actionLabel": "Do It", "route": "POST kuvo/sandbox/somethingElse"}"#).notice
        #expect(unknown.buttons.isEmpty)
    }
}
