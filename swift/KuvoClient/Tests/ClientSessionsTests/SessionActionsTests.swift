import ClientSessions
import Foundation
import Testing

/// A session's menu as its box says it (PROTOCOL.md, "A session's actions"): read the same by every
/// Swift client, Delete's confirm in the box's words, one the client doesn't know read all the same
/// (and left out where it's drawn), and what can't be done now said why.
struct SessionActionsTests {
    @Test func theMenuIsReadAsTheBoxSaysIt() throws {
        let said = Data(#"""
        {"session": "ses_a", "actions": [
          {"id": "pin", "label": "Pin"},
          {"id": "share", "label": "Share"},
          {"id": "delete", "label": "Delete", "destructive": true,
           "confirm": {"title": "Delete “Trip”?", "message": "Its worktree and changes that weren’t committed are deleted too.", "label": "Delete"}}
        ], "held": "It’s moving to another computer. Try again once it’s there."}
        """#.utf8)
        let menu = try JSONDecoder().decode(SessionActions.self, from: said)
        #expect(menu.actions.map(\.id) == ["pin", "share", "delete"])
        #expect(menu.action("pin")?.destructive == false && menu.action("pin")?.confirm == nil)
        #expect(menu.action("delete")?.destructive == true)
        #expect(menu.action("delete")?.confirm?.message == "Its worktree and changes that weren’t committed are deleted too.")
        #expect(menu.held == "It’s moving to another computer. Try again once it’s there.")
        #expect(menu.action("archive") == nil)
    }

    @Test func eachIsAskedOfTheSessionsOwnRoute() {
        #expect(SessionActions.route("ses_a", "actions") == "kuvo/sessions/ses_a/actions")
        #expect(SessionActions.route("ses_a") == "kuvo/sessions/ses_a")
        #expect(SessionActions.route("ses a/b", "pin") == "kuvo/sessions/ses%20a%2Fb/pin")
        #expect(SessionActions.deletesRoute == "kuvo/sessions/deletes")
        let all = try? JSONDecoder().decode(SessionActions.Confirm.self, from: Data(#"{"title": "Delete 2 sessions?", "message": "Their messages and undo snapshots go too. This can’t be undone.", "label": "Delete"}"#.utf8))
        #expect(all?.title == "Delete 2 sessions?")
    }
}
