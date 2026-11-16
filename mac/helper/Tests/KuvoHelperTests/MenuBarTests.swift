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

    /// As a box with two other computers answers (box/bridge-tests/at-work-computers.test.js holds
    /// the box to it): this Mac, a machine at work with a session waiting on you, and a Mac away.
    static let everywhere = """
    {"applies":true,"working":1,"sessions":[{"id":"ses_1","title":"Fix sign-up form validation","bot":"Mira"}],"asking":[],"waiting":{"sessions":0,"bots":0,"queue":0},"background":0,"busy":true,
     "rings":[{"service":"claude","title":"Claude","short":{"used":0.62,"level":"calm"}},{"service":"codex","title":"Codex","short":{"used":0.91,"level":"near"},"long":{"used":0.4,"level":"calm"}}],
     "computers":[
      {"key":"","name":"MacBook Pro","kind":"mac","here":true,"reach":"connected","working":1,"sessions":[{"id":"ses_1","title":"Fix sign-up form validation","bot":"Mira"}],"waiting":0,"asking":[],
       "rings":[{"service":"claude","title":"Claude","short":{"used":0.62,"level":"calm"}}]},
      {"key":"KEYminiKEYminiKEYminiKEYminiKEYminiKEYmini0","name":"mini-local","kind":"mac","reach":"connected","working":2,"sessions":[{"id":"ses_a","title":"Ship the release"},{"id":"ses_b","title":"Nightly dependency bump"}],
       "waiting":1,"asking":[{"id":"ses_q","title":"Deploy preview to staging","asks":"asks to run a command"}],
       "rings":[{"service":"claude","title":"Claude","short":{"used":0.62,"level":"calm"}},{"service":"codex","title":"Codex","short":{"used":0.91,"level":"near"},"long":{"used":0.4,"level":"calm"}}]},
      {"key":"KEYdevboxKEYdevboxKEYdevboxKEYdevboxKEYdevb","name":"devbox","kind":"machine","reach":"away","sessions":[],"asking":[],"rings":[]}
     ]}
    """

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

    /// Every one of your computers (#781): the item counts what's at work on all of them, and its
    /// dot is for a session waiting on any of them; the panel lists each, this Mac first.
    @Test func theItemAndPanelShowEveryComputer() throws {
        let said = try #require(AtWork.read(Data(Self.everywhere.utf8)))
        let look = MenuBarLook(.running(said))
        #expect(look.computers.map(\.name) == ["MacBook Pro", "mini-local", "devbox"])
        #expect(look.computers.map(\.isHere) == [true, false, false])
        #expect(look.count == 3, "one here, two on mini-local")
        #expect(look.needsYou, "a session on mini-local waits on you")
        #expect(look.spoken == "Kuvo, 3 sessions at work, 1 waiting on you")
        // Each computer's own rings: mini-local's Codex isn't this Mac's.
        #expect(look.computers[0].rings.map(\.service) == ["claude"])
        #expect(look.computers[1].rings.map(\.service) == ["claude", "codex"])
        // Away: nothing said of it now.
        #expect(look.computers[2].reach == "away" && look.computers[2].working == nil)
        // Stop is this Mac's alone: it asks of this Mac's one session, not the others'.
        #expect(look.stopQuestion(appOpen: false).detail == "A session is at work. Kuvo stops now, and starts again when you open it.")
        // An older box that lists no computers: this Mac alone, from what it says of itself.
        let older = MenuBarLook(.running(Self.said))
        #expect(older.computers.count == 1)
        #expect(older.computers[0].isHere && older.computers[0].working == 3 && older.computers[0].asking.count == 1)
    }

    /// A click opens the desktop app at the computer, or the session: by a link it takes, this
    /// Mac's by no key (the app's own box), another's by its key.
    @Test func openingAComputerOrASessionIsALinkTheAppTakes() {
        #expect(OpenAt(computer: nil).url.absoluteString == "kuvo-code://open")
        #expect(OpenAt(computer: nil, session: "ses_1").url.absoluteString == "kuvo-code://open?session=ses_1")
        #expect(OpenAt(computer: "KEY-_mini0", session: "ses_q").url.absoluteString == "kuvo-code://open?computer=KEY-_mini0&session=ses_q")
    }

    /// Open Kuvo opens the very copy of the desktop app that told this Mac's Kuvo where it is; where
    /// that's gone (or isn't Kuvo), the newest copy installed, never an older one with the same ID.
    @Test func openKuvoOpensTheAppThatRegisteredElseTheNewestCopy() throws {
        let folder = FileManager.default.temporaryDirectory.appending(path: "kuvo-desktop-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: folder) }
        func app(_ name: String, id: String = "run.kuvo.code", version: String, build: String) throws -> URL {
            let url = folder.appending(path: "\(name)/Kuvo.app")
            try FileManager.default.createDirectory(at: url.appending(path: "Contents/MacOS"), withIntermediateDirectories: true)
            let info: [String: Any] = ["CFBundleIdentifier": id, "CFBundleShortVersionString": version, "CFBundleVersion": build, "CFBundleExecutable": "kuvo", "CFBundlePackageType": "APPL"]
            try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0).write(to: url.appending(path: "Contents/Info.plist"))
            return url
        }
        let old = try app("old", version: "0.9.180", build: "0.9.180")
        let beta = try app("beta", version: "1.0.0-beta.2", build: "1.0.0b2")
        let release = try app("release", version: "1.0.0", build: "1.0.0")
        let other = try app("other", id: "com.example.other", version: "9.9.9", build: "9.9.9")
        let all = [old, release, beta, other]
        // The registered copy, though a newer one is installed.
        #expect(DesktopApp.pick(registered: beta, copies: all, app: "run.kuvo.code", trusted: { _ in true }) == beta)
        // None registered, or it's gone, another app, or not Kuvo's: the newest copy (a release above its betas).
        #expect(DesktopApp.pick(registered: nil, copies: [old, beta, release], app: "run.kuvo.code", trusted: { _ in true }) == release)
        #expect(DesktopApp.pick(registered: folder.appending(path: "gone/Kuvo.app"), copies: [old, beta], app: "run.kuvo.code", trusted: { _ in true }) == beta)
        #expect(DesktopApp.pick(registered: other, copies: [old, beta], app: "run.kuvo.code", trusted: { _ in true }) == beta)
        #expect(DesktopApp.pick(registered: old, copies: [old, beta], app: "run.kuvo.code", trusted: { $0 != old }) == beta)
        // Nothing of Kuvo's installed.
        #expect(DesktopApp.pick(registered: nil, copies: [other], app: "run.kuvo.code", trusted: { _ in true }) == nil)
    }

    @Test func stopAsksFirstWhileTheBoxSaysSomethingWouldStopOrDoesntSayYetOrTheAppIsOpen() {
        #expect(MenuBarLook(.running(Self.said)).stopAsks(appOpen: false))
        #expect(MenuBarLook(.running(Self.said)).stopQuestion(appOpen: false).detail == "3 sessions are at work. Kuvo stops now, and starts again when you open it.")
        #expect(!MenuBarLook(.running(Self.quiet)).stopAsks(appOpen: false), "nothing at work: it just stops")
        var background = Self.quiet
        background.busy = true
        #expect(MenuBarLook(.running(background)).stopAsks(appOpen: false), "the box's word, not a count of this item's")
        #expect(MenuBarLook(.starting).stopAsks(appOpen: false), "not known yet: asked")
        // The desktop app open: its window closes too, so it's asked, and said.
        #expect(MenuBarLook(.running(Self.quiet)).stopAsks(appOpen: true))
        #expect(MenuBarLook(.running(Self.quiet)).stopQuestion(appOpen: true).detail == "Kuvo stops now, and its window closes. It starts again when you open Kuvo.")
        #expect(!MenuBarLook(.stopping).canStop)
    }

    @Test func theDotIsForAnythingWaitingOnYouInAnyArea() {
        for waiting in [AtWork.Waiting(sessions: 0, bots: 1, queue: 0), AtWork.Waiting(sessions: 0, bots: 0, queue: 2)] {
            var said = Self.quiet
            said.waiting = waiting
            #expect(MenuBarLook(.running(said)).needsYou)
        }
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

    /// The mark is the committed asset, drawn as a template at the menu bar's size, so macOS tints
    /// it for a light or dark menu bar and when it's selected, as its own icons.
    @Test func theMarkIsTheAssetAsATemplate() throws {
        let file = URL(filePath: #filePath).deletingLastPathComponent().appending(path: "../../Assets/kuvo-mark.png").standardizedFileURL
        #expect(try Data(contentsOf: file) == MenuBarMark.png, "mac/helper/Assets/kuvo-mark.png and the bytes the program carries")
        let image = MenuBarMark.image()
        #expect(image.isTemplate)
        #expect(image.size == NSSize(width: 18, height: 18))
        #expect(image.representations.first?.pixelsWide == 54)
    }

    /// Each service the box reads limits of has its mark, the desktop app's own file, as a template;
    /// one Kuvo has no mark for (or an id that isn't one) has none, and keeps its letter.
    @Test @MainActor func theRingsShowEachServicesMark() {
        for service in ["claude", "codex", "grok", "cursor", "antigravity", "copilot", "openrouter", "zai", "ollama", "opencode"] {
            let mark = ServiceMark.image(service)
            #expect(mark != nil, "\(service)")
            #expect(mark?.isTemplate == true)
        }
        #expect(ServiceMark.image("newservice") == nil)
        #expect(ServiceMark.image("../marks/claude") == nil)
    }

    /// The item as a menu bar draws it (the mark tinted as a template is, the count, the dot), on a
    /// light and a dark menu bar; written out where `HELPER_SHOTS` names a folder.
    @Test func theItemDrawsOnLightAndDarkMenuBars() throws {
        guard let folder = ProcessInfo.processInfo.environment["HELPER_SHOTS"].map({ URL(filePath: $0) }) else { return }
        let looks = [MenuBarLook(.running(Self.said)), MenuBarLook(.running(Self.quiet))]
        let strip = HStack(spacing: 12) {
            ForEach([false, true], id: \.self) { dark in
                VStack(alignment: .trailing, spacing: 6) {
                    ForEach(looks.indices, id: \.self) { index in
                        let look = looks[index]
                        HStack(spacing: 3) {
                            Image(nsImage: MenuBarMark.image()).renderingMode(.template)
                            if let count = look.count { Text(" \(count)").font(.system(size: 12, weight: .medium).monospacedDigit()) }
                            if look.needsYou { Text("●").font(.system(size: 7)).foregroundStyle(.orange).baselineOffset(3) }
                        }
                        .foregroundStyle(dark ? Color.white : Color.black)
                        .padding(.horizontal, 6).frame(height: 22)
                        .background(RoundedRectangle(cornerRadius: 4).fill(index == 0 ? (dark ? Color.white : Color.black).opacity(0.14) : .clear))
                    }
                }
                .padding(10).frame(width: 140, alignment: .trailing)
                .background(dark ? Color(white: 0.11) : Color(white: 0.965))
            }
        }
        let renderer = ImageRenderer(content: strip)
        renderer.scale = 3
        let image = try #require(renderer.nsImage)
        let png = try #require(image.tiffRepresentation.flatMap { NSBitmapImageRep(data: $0)?.representation(using: .png, properties: [:]) })
        try png.write(to: folder.appending(path: "item-light-dark.png"))
    }

    /// The panel drawn in each state, light and dark, at its width; written out where
    /// `HELPER_SHOTS` names a folder (for a look before and after).
    @Test func thePanelDrawsInEveryState() throws {
        var long = Self.said
        long.sessions = (1...9).map { AtWork.Session(id: "ses_\($0)", title: "A session with a title much too long to fit on one line of the panel \($0)", bot: $0 % 2 == 0 ? "A bot with a long name" : nil) }
        long.working = 14
        // Many computers, each with much at work: the list scrolls past its tallest.
        var crowded = try #require(AtWork.read(Data(Self.everywhere.utf8)))
        crowded.computers = (1...6).map { n in AtWork.Computer(key: "key\(n)", name: "Computer with a rather long name \(n)", kind: n % 2 == 0 ? "mac" : "machine", here: n == 1 ? true : nil, reach: "connected",
                                                               working: 9, sessions: long.sessions, waiting: 0, asking: [], rings: Self.said.rings) }
        let states: [(String, MenuBarLook)] = [
            ("working", MenuBarLook(.running(Self.said))), ("idle", MenuBarLook(.running(Self.quiet))), ("starting", MenuBarLook(.starting)),
            ("stopping", MenuBarLook(.stopping)), ("many", MenuBarLook(.running(long))),
            ("computers", MenuBarLook(.running(try #require(AtWork.read(Data(Self.everywhere.utf8)))))), ("crowded", MenuBarLook(.running(crowded))),
        ]
        let folder = ProcessInfo.processInfo.environment["HELPER_SHOTS"].map { URL(filePath: $0) }
        for (name, look) in states {
            for dark in [false, true] {
                let panel = MenuBarPanel(look: look, actions: MenuBarActions(open: { _ in }, stop: {})).environment(\.colorScheme, dark ? .dark : .light)
                let renderer = ImageRenderer(content: panel)
                renderer.scale = 2
                let image = try #require(renderer.nsImage)
                #expect(image.size.width == 300, "\(name): the panel keeps its width")
                #expect(image.size.height <= MenuBarPanel.tallest + 120, "\(name): its computers scroll past their tallest")
                // The height it works out ahead (to scroll only past its tallest) is the one drawn.
                if look.atWork != nil {
                    let computers = MenuBarPanel.height(of: look.computers, most: MenuBarPanel.most(of: look.computers.count))
                    let drawn = min(computers, MenuBarPanel.tallest) + 6 * 2 + 9 + 28 * 2
                    #expect(abs(image.size.height - drawn) <= 2, "\(name): \(image.size.height) drawn, \(drawn) worked out")
                }
                if let folder, let tiff = image.tiffRepresentation, let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) {
                    try png.write(to: folder.appending(path: "panel-\(name)-\(dark ? "dark" : "light").png"))
                }
            }
        }
    }
}
#endif
