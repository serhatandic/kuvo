#if os(macOS)
import AppKit
import Foundation
import SwiftUI
import Testing
@testable import KuvoHelperKit

/// Kuvo in the menu bar: what the item and its panel show from what the box says
/// (`kuvo/box/at-work`, PROTOCOL.md "In the menu bar"), and the server it reads that from.
@MainActor
@Suite struct MenuBarTests {
    /// As the box answers (box/test/at-work.test.ts holds the box to the same shape).
    static let answer = """
    {"applies":true,"working":3,"sessions":[{"id":"ses_1","title":"Fix sign-up form validation","bot":"Mira"},{"id":"ses_2","title":"Nightly dependency bump"},{"id":"ses_3","title":"Write the release notes","bot":"Otto"}],
     "asking":[{"id":"ses_4","title":"Deploy preview to staging","asks":"asks to run a command"}],"waiting":{"sessions":1,"bots":0,"queue":0},"background":1,"busy":true,
     "rings":[{"service":"claude","title":"Claude","short":{"used":0.62,"level":"calm"},"long":{"used":0.34,"level":"calm"}},{"service":"codex","title":"Codex","short":{"used":0.81,"level":"near"}}],"newer":"left as it is"}
    """

    static var said: AtWork { AtWork.read(Data(answer.utf8))! }

    static let quiet = AtWork(applies: true, working: 0, sessions: [], asking: [], waiting: .init(sessions: 0, bots: 0, queue: 0), background: 0, busy: false, rings: [])

    @Test func theItemShowsHowManySessionsWorkAndWhetherSomethingWaitsOnYou() {
        let look = MenuBarLook(.running(Self.said))
        #expect(look.count == 3)
        #expect(look.needsYou)
        #expect(look.spoken == "Kuvo, 3 sessions at work, 1 waiting on you")
        let idle = MenuBarLook(.running(Self.quiet))
        #expect(idle.count == nil, "no count while nothing works")
        #expect(!idle.needsYou)
        #expect(idle.spoken == "Kuvo, nothing at work")
        // Before the box answers, nothing is claimed.
        #expect(MenuBarLook(.starting).count == nil)
        #expect(MenuBarLook(.starting).spoken == "Kuvo, starting")
    }

    @Test func stopAsksFirstWhileTheBoxSaysSomethingWouldStopOrDoesntSayYet() {
        #expect(MenuBarLook(.running(Self.said)).stopAsks)
        #expect(MenuBarLook(.running(Self.said)).stopQuestion.detail.hasPrefix("3 sessions are at work."))
        #expect(!MenuBarLook(.running(Self.quiet)).stopAsks, "nothing at work: it just stops")
        var background = Self.quiet
        background.busy = true
        #expect(MenuBarLook(.running(background)).stopAsks, "the box's word, not a count of this item's")
        #expect(MenuBarLook(.starting).stopAsks, "not known yet: asked")
        #expect(!MenuBarLook(.stopping).canStop)
    }

    @Test func anAnswerThisCantReadIsLeftOut() {
        #expect(AtWork.read(Data(#"{"applies":true}"#.utf8)) == nil)
        #expect(AtWork.read(Data("not json".utf8)) == nil)
        #expect(Self.said.rings[1].long == nil, "an account with one ring has one")
    }

    /// Only the server the helper started is read from: a record naming another password (one a
    /// crash left, its port maybe someone else's) isn't.
    @Test func onlyTheServerTheHelperStartedIsReached() throws {
        let run = FileManager.default.temporaryDirectory.appending(path: "kuvo-menu-bar-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: run, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: run) }
        try Data(#"{"pid":123,"port":40123,"password":"ours"}"#.utf8).write(to: run.appending(path: "serve.json"))
        let server = try #require(BoxServer.recorded(in: run, password: "ours"))
        #expect(server.port == 40123)
        let request = server.request("kuvo/box/at-work/events")
        #expect(request.url?.absoluteString == "http://127.0.0.1:40123/kuvo/box/at-work/events")
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Basic " + Data("opencode:ours".utf8).base64EncodedString())
        #expect(BoxServer.recorded(in: run, password: "another") == nil)
        #expect(BoxServer.recorded(in: run, password: nil) == nil, "before it started one")
    }

    /// The panel drawn in each state, light and dark, at its width; written out where
    /// `HELPER_SHOTS` names a folder (for a look before and after).
    @Test func thePanelDrawsInEveryState() throws {
        var long = Self.said
        long.sessions = (1...9).map { AtWork.Session(id: "ses_\($0)", title: "A session with a title much too long to fit on one line of the panel \($0)", bot: $0 % 2 == 0 ? "A bot with a long name" : nil) }
        long.working = 14
        let states: [(String, MenuBarLook)] = [
            ("working", MenuBarLook(.running(Self.said))), ("idle", MenuBarLook(.running(Self.quiet))), ("starting", MenuBarLook(.starting)),
            ("stopping", MenuBarLook(.stopping)), ("many", MenuBarLook(.running(long))),
        ]
        let folder = ProcessInfo.processInfo.environment["HELPER_SHOTS"].map { URL(filePath: $0) }
        for (name, look) in states {
            for dark in [false, true] {
                let panel = MenuBarPanel(look: look, actions: MenuBarActions(open: {}, stop: {})).environment(\.colorScheme, dark ? .dark : .light)
                let renderer = ImageRenderer(content: panel)
                renderer.scale = 2
                let image = try #require(renderer.nsImage)
                #expect(image.size.width == 300, "\(name): the panel keeps its width")
                if let folder, let tiff = image.tiffRepresentation, let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) {
                    try png.write(to: folder.appending(path: "panel-\(name)-\(dark ? "dark" : "light").png"))
                }
            }
        }
    }
}
#endif
