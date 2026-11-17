#if os(macOS)
import AppKit
import SwiftUI

/// A service's mark in the panel's rings, by the box's `service` id: the desktop app's own files
/// (gpui/assets/marks), which scripts/build-helper.sh puts in the helper's Resources/Marks, drawn
/// as a template so they follow light and dark. None for an id Kuvo has no mark for.
public enum ServiceMark {
    /// Where the marks are: the helper's bundle; a test build, the desktop app's folder in the source.
    nonisolated(unsafe) public static var folder: URL? = {
        if let inBundle = Bundle.main.resourceURL?.appending(path: "Marks"), FileManager.default.fileExists(atPath: inBundle.path) { return inBundle }
        #if DEBUG
        return URL(filePath: #filePath).deletingLastPathComponent().appending(path: "../../../../gpui/assets/marks").standardizedFileURL
        #else
        return nil
        #endif
    }()

    @MainActor private static var loaded: [String: NSImage?] = [:]

    @MainActor public static func image(_ service: String) -> NSImage? {
        // (An id is the box's word: letters, digits and dashes, never a path.)
        guard !service.isEmpty, service.allSatisfy({ $0.isASCII && ($0.isLowercase || $0.isNumber || $0 == "-") }), let folder else { return nil }
        if let known = loaded[service] { return known }
        let image = NSImage(contentsOf: folder.appending(path: "\(service).svg"))
        image?.isTemplate = true
        loaded[service] = image
        return image
    }
}

/// Kuvo's mark in the menu bar: the app icon's artwork made one colour, with thin gaps where its
/// shapes meet (the user's pick, 11-16), as a template, so macOS draws it as it draws its own icons
/// (light or dark menu bar, selected or not). Its bytes are mac/helper/Assets/kuvo-mark.png's
/// (made by .claude/notes/761-drafts/logo.swift; a test holds them to the file), carried in the
/// program as the helper's bundle has no resources of its own.
public enum MenuBarMark {
    /// The PNG, 54 px square: 18 points at 3x.
    static let png = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAADYAAAA2CAYAAACMRWrdAAAAAXNSR0IArs4c6QAAADhlWElmTU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAAqACAAQAAAABAAAANqADAAQAAAABAAAANgAAAADQiwyeAAAHF0lEQVRoBe2Xe4jVRRTHfaWm6yMfWYayalptq0JmCaYpWoSa9JKyCASjjEysxNC0aHtR2hMsybLsn9CkwqxMDbV8hJoSafl+ZWam+Ugz11ef7905y+xv7+93f/fuz/BKBz57zsycOTNzZ+bMb6tV+1/y6xeonsB0axDjaiiC47AGfoC8lk7Mfj6chNOOUvQX0BnyUrow691gCwrqP2jrneDK2hFrGJTAUGgJiUsjIurIBRcTLGtxOqZVkbp01mL2gx//V8p3Q6LyDNH8QaLsXfhqAg1zmEEhfXTUw+KfoC2xxenu/BUxWNgkttFnKcyB1+F6iJK+NG6FsHhWfwCfjlGB4rS1wWk1WNCqaCWcCaCsGpQRVPwNceMvwrd2MEimsp4E/SJPwE6IO1hcPyUEk3oYkyFuX99vuAWJo+/AaTEcAz9IkvZaYtcCnYavqzDOHvq2gkjR8XgFklxAulg7GKO/m8msBMab6mKFqtEJDJJuIX7dSsa4zM1AKX19AmPqy6eHi1lJFVKjTONPImlbKbwZSBrDdEhqjG+IpbxQSUZSk8sgB+mnd8X67sPWHVXGsjppfUc2BUl7WA5+exL2eAUPyjQqsg2uRVwBo7y+n2NLbgKLV4rdU5VIB9gC1pa01hgVZAalbAdZ4iK0QNunz8euri3a3qS5rk5vTtQXhcbX7r8I30G28zmt7BeUTcGKGGWLo53b5fxbOm3HU8WvXF13dB9nhyn9h/A4vO85zMPW85NRbEK+46de4aRnR5mtaSwALcIurh51Hc9iOB8k9qNZNiyrTf/XdryRa96OvgX0oZBR9DAGRZd5MOhI6devB0Wg96Yf2CQxy0W7MwRWQSFI1E+/8Hlgi1Wd5HCZivy72bX2dlq7rSOtY5yTNIzodRVtiyDdmdfu2l1K1666l0HSDrS4MD/VKxENhGPO70G0ZAz4/Y5S/hH0wM+EhbADKsgQSjouH8FoeAlKYADUAUkD+Az84HFtxa4PkikQ1U8/1CnP53nsZrDR1f2E1uJ13HUqfCnfHA02FaIGWkp7Z9dbi1uWwT8Yay/+a6AbSHQvdXQPQdA3XfkwfjvhODwJmoMvuotNwDYgdelnU5EuWLDuN/yuBEkniPq/TEdEx3Ys9ISLwL9vFFP3tT26P2hHVoB2KjiulQ/QJl+TQoxxoEypRe+G9ZB6QxXMOsbR2ilLIK+m6buNOh3fjpBOalOpJFLgtH+MFLcHvAPBH02L6gUmD2PsgbA5hzaEdVD9cBe9LfqIC74drTPfFHzRzo6AD2AJrIPNsBV+hi/hadCu+sermPK7cAI05mCQKJO/BVHzU1tGBwugndU2qyxtqfs97AnQHExaYDwEWsgxsBiZ9Gp8HwN73Gtg65OsBEw0VqY4ao/lpAur7HM52BG5FVuio2WixY0Hnfc4g4f5aPdHghKCL7dTCOsTrE/r+A8BJsEWF+gkWm+YZAGcgntVcKJ7oh3ShIIDVKWs78TuYDIIQ9kxTsy0Tt+7SAPQWpQC6YhIJsJdKavsT1fUQogzWC4+R4itO2pyI8Z+8GMpMz4VqKvgYM56/GqC7pHtglK9Ml0BSKqDFhv3F7TYuerXGMsyaF9sW9wGbN3F3qDYpTAD0i5Mjd3UiKwCdVAiaAuSC2A65DrJXPvNZMz6IOkHSkyqkzwCivumCpKwQbSQoaDOSsmNQdIa9JaF9TvT9fpBLWE9gD0XWsJa0N3vASnJNJF5eDV0vsqKCpCpz5lun8wcdBUkegr2gsbUm9ceUhI1icV46PtL0gksS0b1+a/aHk3NquzbcAG2jfshtjajvMIaTK+hTVssKYIkFlXV983mJn0UeoGkA+wDv71CwRp+x6kYJPrfaQNYW676OWIo6UwLxNK9ULq+GexIxR1jHX3sROlB9/tVKKhBXxkDQaJOK8HvkIu9mxjNQKJsq8UojvS3YDIJI9v4b7jOddErvP6VAr3gHPU26D3IdqCg/0FiTHExpdqAHl35/QKjwOR+jGD/TGVlbXuaBlh/Td4XfXE86yr0+A7yG3O09eWi829SC6OmK9RBH7IGtCaZrSj1KzNqLXqWlPBSYr9IKaXrXF0XtP2q1l4VPcfFlboB/FialMlYDL8trq0j3ccFuc3FKA/0tmvQL6BVxw2ayU9vyza4ECQ6ln6fWanasj/zA22+XyZ7touju7ZJtjroqLRSARkGmYLk0q5dmwg6bn5/nYw74T5Q4vLbsrGV/vUsSe7RH90BDShpDrrQ2QQ8m3zHaREmYzA6u4Lss2mi2c5lGfO3xJT6DlShADbm+cKUYS+BVIpUQcexK1wK+SwNmPy1WoD/jtkjl88L09wnwSe2iIsx8jlpVLqLtmN/srB1tspzTV/DgvT1UWn1+VhXnhqZ/C7QA1cM55w0YUXLIe93ze6Y7ZDu2lor5LP+F7RBnzb6gO5iAAAAAElFTkSuQmCC")!

    /// The size it's drawn at, as the menu bar's own icons are.
    static let points = NSSize(width: 18, height: 18)

    public static func image() -> NSImage {
        let image = NSImage(data: png) ?? NSImage()
        image.size = points
        image.isTemplate = true
        image.accessibilityDescription = "Kuvo"
        return image
    }
}

/// The desktop app's colours (gpui/kuvo_ui/src/theme.rs), light and dark: the panel is drawn in
/// the app's own look, not as a menu of the system's.
struct Palette {
    let dark: Bool

    private func hex(_ value: UInt32, _ alpha: Double = 1) -> Color {
        Color(.sRGB, red: Double((value >> 16) & 255) / 255, green: Double((value >> 8) & 255) / 255, blue: Double(value & 255) / 255, opacity: alpha)
    }

    var card: Color { dark ? hex(0x1E1E1E) : hex(0xFFFFFF) }
    var text: Color { dark ? hex(0xECECEC) : hex(0x292929) }
    var muted: Color { dark ? hex(0xA3A3A3) : hex(0x5D5D5D) }
    var faint: Color { dark ? hex(0x6E6E6E) : hex(0x9E9E9E) }
    var icon: Color { dark ? hex(0x8F8F8F) : hex(0x7D7D7D) }
    var border: Color { dark ? hex(0x262626) : hex(0xEBEBEB) }
    var hover: Color { dark ? hex(0xFFFFFF, 0.05) : hex(0x292929, 0.04) }
    var success: Color { dark ? hex(0x4CC27A) : hex(0x2E9E5B) }
    var warning: Color { dark ? hex(0xE0A43A) : hex(0xC7820E) }
    var danger: Color { dark ? hex(0xF06363) : hex(0xD93B3B) }

    /// A ring's colour, by the box's word for how close it is: a word this doesn't know is quiet.
    func level(_ word: String) -> Color {
        switch word {
        case "calm": success
        case "near": warning
        case "over": danger
        default: muted
        }
    }
}

/// What the panel's buttons do.
public struct MenuBarActions {
    /// Opens the desktop app; at one of your computers, or a session there, where it's given.
    public var open: (OpenAt?) -> Void
    public var stop: () -> Void

    public init(open: @escaping (OpenAt?) -> Void, stop: @escaping () -> Void) {
        self.open = open
        self.stop = stop
    }
}

/// The item's panel (option A, picked 11-16; every computer, option B, #781): each of your
/// computers, this Mac first, with whether it's reached, what needs you there, what's at work (with
/// the bot it works for) and the rings of the accounts signed in there; a click opens the desktop
/// app at that computer or session. Then Open Kuvo, and Stop, for this Mac alone.
public struct MenuBarPanel: View {
    let look: MenuBarLook
    let actions: MenuBarActions
    @Environment(\.colorScheme) private var scheme

    /// The most rows of each kind it lists for a computer: the rest are counted ("2 more"); fewer
    /// for each where there are several.
    static func most(of computers: Int) -> Int { computers > 1 ? 3 : 6 }

    /// The tallest its computers get before they scroll.
    static let tallest: CGFloat = 460

    /// How tall the computers are drawn, from their rows' fixed heights.
    static func height(of computers: [AtWork.Computer], most: Int) -> CGFloat {
        computers.enumerated().reduce(0) { height, each in
            let (index, computer) = each
            let asking = computer.asking.prefix(most).count, sessions = computer.sessions.prefix(most).count
            var rows = CGFloat(30 + 2) + CGFloat(asking + sessions) * 28 + (index > 0 ? 9 : 0)
            if asking > 0, max(computer.waiting ?? 0, computer.asking.count) > asking { rows += 22 }
            if sessions > 0, (computer.working ?? 0) > sessions { rows += 22 }
            if asking + sessions == 0, computer.isHere || computer.reach != "away" { rows += 24 }
            let accounts = min(computer.rings.count, 6)
            if accounts > 0 { rows += CGFloat((accounts + 1) / 2) * 37 + 4 }
            return height + rows
        }
    }

    public init(look: MenuBarLook, actions: MenuBarActions) {
        self.look = look
        self.actions = actions
    }

    public var body: some View {
        let p = Palette(dark: scheme == .dark)
        VStack(alignment: .leading, spacing: 0) {
            if look.atWork == nil {
                HStack {
                    Text("Kuvo on this Mac").font(.system(size: 13, weight: .semibold)).foregroundStyle(p.text)
                    Spacer()
                    Status(word: look.status, color: look.stage == .stopping ? p.faint : p.warning, p: p)
                }
                .padding(.horizontal, 10).frame(height: 30)
            } else {
                let most = Self.most(of: look.computers.count)
                let list = VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(look.computers.enumerated()), id: \.element.key) { index, computer in
                        if index > 0 { Divider(p: p) }
                        ComputerSection(computer: computer, look: look, most: most, p: p, open: actions.open)
                    }
                }
                // (Scrolled only past its tallest: every row's height is fixed, so that's known ahead.)
                if Self.height(of: look.computers, most: most) > Self.tallest {
                    ScrollView { list }.frame(height: Self.tallest)
                } else {
                    list
                }
            }
            Divider(p: p)
            ActionRow(symbol: "macwindow", title: "Open Kuvo", shortcut: "⌘O", tint: p.text, p: p) { actions.open(nil) }
                .keyboardShortcut("o", modifiers: .command)
            ActionRow(symbol: "stop.circle", title: "Stop Kuvo on this Mac…", shortcut: nil, tint: p.text, p: p, action: actions.stop)
                .disabled(!look.canStop)
                .opacity(look.canStop ? 1 : 0.5)
        }
        .padding(6)
        .frame(width: 300)
        .background(p.card)
    }
}

/// A dot and a word: how a computer stands.
private struct Status: View {
    let word: String?
    let color: Color
    let p: Palette
    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(color).frame(width: 6, height: 6)
            if let word { Text(word).font(.system(size: 12)).foregroundStyle(p.muted) }
        }
        .accessibilityElement(children: .combine)
    }
}

/// One of your computers: its name and how it stands (a click opens it in the desktop app), what
/// needs you there, what's at work, and its accounts' rings.
private struct ComputerSection: View {
    let computer: AtWork.Computer
    let look: MenuBarLook
    let most: Int
    let p: Palette
    let open: (OpenAt?) -> Void

    /// Where the desktop app opens it: this Mac's by no key (the app's nearest), another's by its.
    private func at(_ session: String? = nil) -> OpenAt { OpenAt(computer: computer.isHere ? nil : computer.key, session: session) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Hovered(p: p, action: { open(at()) }) {
                HStack(spacing: 7) {
                    Image(systemName: computer.kind == "mac" ? "laptopcomputer" : "server.rack").font(.system(size: 11, weight: .regular)).foregroundStyle(p.icon).frame(width: 14)
                    Text(computer.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(p.text).lineLimit(1).truncationMode(.middle)
                    if computer.isHere { Text("This Mac").font(.system(size: 12)).foregroundStyle(p.faint).lineLimit(1).fixedSize() }
                    Spacer(minLength: 8)
                    standing
                }
                .padding(.horizontal, 10).frame(height: 30)
            }
            .accessibilityLabel(spokenName)
            .accessibilityHint("Opens it in Kuvo")
            if !computer.asking.isEmpty {
                ForEach(computer.asking.prefix(most), id: \.id) { asking in
                    Hovered(p: p, action: { open(at(asking.id)) }) {
                        Line(title: asking.title ?? "A session", detail: nil, p: p) { Circle().fill(p.warning).frame(width: 7, height: 7) }
                    }
                    .accessibilityLabel("\(asking.title ?? "A session"), \(asking.asks ?? "waits on you")")
                    .help(asking.asks ?? "")
                }
                More(count: max(computer.waiting ?? 0, computer.asking.count) - min(computer.asking.count, most), p: p)
            }
            if !computer.sessions.isEmpty {
                ForEach(computer.sessions.prefix(most), id: \.id) { session in
                    Hovered(p: p, action: { open(at(session.id)) }) {
                        Line(title: session.title ?? "A session", detail: session.bot, p: p) { Spinner(p: p) }
                    }
                    .accessibilityLabel([session.title ?? "A session", "at work", session.bot.map { "for \($0)" }].compactMap { $0 }.joined(separator: ", "))
                }
                More(count: (computer.working ?? 0) - min(computer.sessions.count, most), p: p)
            }
            // (Away says so beside its name: nothing more under it.)
            if computer.asking.isEmpty, computer.sessions.isEmpty, computer.isHere || computer.reach != "away" {
                Text(quiet).font(.system(size: 12)).foregroundStyle(p.faint)
                    .padding(.leading, 33).padding(.trailing, 10).frame(height: 24, alignment: .leading)
            }
            if !computer.rings.isEmpty {
                Accounts(accounts: computer.rings, p: p)
            }
        }
        .padding(.bottom, 2)
    }

    /// How it stands: this Mac's Kuvo running; another reached (a dot alone), being reached, or away.
    @ViewBuilder private var standing: some View {
        if computer.isHere {
            Status(word: look.status, color: look.stage == .stopping ? p.faint : p.success, p: p)
        } else {
            switch computer.reach {
            case "connected": Status(word: nil, color: p.success, p: p)
            case "connecting": Status(word: "Connecting…", color: p.warning, p: p)
            default: Status(word: "Away", color: p.faint, p: p)
            }
        }
    }

    private var spokenName: String {
        let standing = computer.isHere ? "this Mac, \(look.status)" : computer.reach == "connected" ? "connected" : computer.reach == "connecting" ? "connecting" : "away"
        return "\(computer.name), \(standing)"
    }

    /// What's said where nothing is listed.
    private var quiet: String {
        if computer.working == nil { return computer.reach == "connecting" ? "Reaching it…" : "Update Kuvo there to see its work" }
        if computer.isHere, (look.atWork?.background ?? 0) > 0 { return "Work is running in the background" }
        return "Nothing at work"
    }
}

/// A row that opens something: highlighted under the pointer, as the panel's actions are.
private struct Hovered<Content: View>: View {
    let p: Palette
    let action: () -> Void
    @ViewBuilder let content: () -> Content
    @State private var hovered = false
    var body: some View {
        Button(action: action) {
            content()
                .contentShape(Rectangle())
                .background(RoundedRectangle(cornerRadius: 6).fill(hovered ? p.hover : .clear))
        }
        .buttonStyle(.plain)
        .onHover { hovered = $0 }
    }
}

private struct Divider: View {
    let p: Palette
    var body: some View { Rectangle().fill(p.border).frame(height: 1).padding(.horizontal, 10).padding(.vertical, 4) }
}

private struct More: View {
    let count: Int
    let p: Palette
    var body: some View {
        if count > 0 {
            Text("\(count) more").font(.system(size: 12)).foregroundStyle(p.faint).padding(.leading, 33).frame(height: 22)
        }
    }
}

private struct Line<Lead: View>: View {
    let title: String
    let detail: String?
    let p: Palette
    @ViewBuilder let lead: () -> Lead
    var body: some View {
        HStack(spacing: 9) {
            lead().frame(width: 14)
            Text(title).font(.system(size: 13)).foregroundStyle(p.text).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 8)
            // (The bot's name keeps up to a third of the row; the title gives way first.)
            if let detail { Text(detail).font(.system(size: 12)).foregroundStyle(p.faint).lineLimit(1).truncationMode(.tail).frame(maxWidth: 96, alignment: .trailing).layoutPriority(1) }
        }
        .padding(.horizontal, 10).frame(height: 28)
        .accessibilityElement(children: .combine)
    }
}

private struct ActionRow: View {
    let symbol: String
    let title: String
    let shortcut: String?
    let tint: Color
    let p: Palette
    let action: () -> Void
    @State private var hovered = false
    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                Image(systemName: symbol).font(.system(size: 12, weight: .light)).foregroundStyle(p.icon).frame(width: 14)
                Text(title).font(.system(size: 13)).foregroundStyle(tint)
                Spacer()
                if let shortcut { Text(shortcut).font(.system(size: 12)).foregroundStyle(p.faint) }
            }
            .padding(.horizontal, 10).frame(height: 28)
            .contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: 6).fill(hovered ? p.hover : .clear))
        }
        .buttonStyle(.plain)
        .onHover { hovered = $0 }
    }
}

/// A turning arc, as the desktop app's spinner.
private struct Spinner: View {
    let p: Palette
    @Environment(\.accessibilityReduceMotion) private var still
    var body: some View {
        TimelineView(.animation(paused: still)) { time in
            let turn = still ? 0 : time.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 1) * 360
            ZStack {
                Circle().stroke(p.border, lineWidth: 1.5)
                Arc(used: 0.28).stroke(p.icon, style: StrokeStyle(lineWidth: 1.5, lineCap: .round)).rotationEffect(.degrees(turn))
            }
            .frame(width: 11, height: 11)
        }
        .accessibilityHidden(true)
    }
}

struct Arc: Shape {
    var used: Double
    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.addArc(center: CGPoint(x: rect.midX, y: rect.midY), radius: rect.width / 2, startAngle: .degrees(-90), endAngle: .degrees(-90 + 360 * min(max(used, 0), 1)), clockwise: false)
        return path
    }
}

/// Each account's rings, as the desktop app's rail draws them: the short window outside, the long
/// one inside, each filled with what's left of it, coloured by the box's word; its name, and
/// what's left of the short window.
private struct Accounts: View {
    let accounts: [AtWork.Account]
    let p: Palette
    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 120), alignment: .leading)], alignment: .leading, spacing: 8) {
            ForEach(Array(accounts.prefix(6).enumerated()), id: \.offset) { _, account in
                HStack(spacing: 7) {
                    Rings(account: account, p: p)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(account.title).font(.system(size: 12, weight: .medium)).foregroundStyle(p.text).lineLimit(1)
                        if let shown = account.short ?? account.long {
                            Text("\(Int(((1 - min(max(shown.used, 0), 1)) * 100).rounded()))% left").font(.system(size: 11).monospacedDigit())
                                .foregroundStyle(shown.level == "calm" ? p.faint : p.level(shown.level))
                        }
                    }
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(Self.spoken(account))
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
    }

    static func spoken(_ account: AtWork.Account) -> String {
        let shares = [account.short, account.long].compactMap { $0 }.map { "\(Int(((1 - min(max($0.used, 0), 1)) * 100).rounded())) percent left" }
        return ([account.title] + shares).joined(separator: ", ")
    }
}

private struct Rings: View {
    let account: AtWork.Account
    let p: Palette
    let size: CGFloat = 26
    let stroke: CGFloat = 2.5
    var body: some View {
        let rings = [account.short, account.long].compactMap { $0 }
        ZStack {
            if rings.isEmpty { ring(nil, size) }
            ForEach(Array(rings.enumerated()), id: \.offset) { index, ring in
                self.ring(ring, size - (stroke + 1.5) * 2 * CGFloat(index))
            }
            // The service's mark, as the desktop app's rail has it; one Kuvo has no mark for (a
            // service a newer box names), its first letter.
            if let mark = ServiceMark.image(account.service) {
                // The files are framed so a mark's size is its slot's (gpui/kuvo_ui/src/icon.rs, `Mark`):
                // 63% of the circle inside the innermost ring, as the desktop app's rail draws it.
                let inner = size - (stroke + 1.5) * 2 * CGFloat(max(rings.count - 1, 0)) - stroke * 2
                Image(nsImage: mark).renderingMode(.template).resizable().scaledToFit()
                    .frame(width: inner * 0.63, height: inner * 0.63).foregroundStyle(p.muted)
            } else {
                Text(account.service.prefix(1).uppercased()).font(.system(size: rings.count > 1 ? 7 : 9, weight: .semibold)).foregroundStyle(p.muted)
            }
        }
        .frame(width: size, height: size)
    }

    private func ring(_ ring: AtWork.Ring?, _ side: CGFloat) -> some View {
        ZStack {
            Circle().stroke(p.border, lineWidth: stroke)
            if let ring { Arc(used: 1 - ring.used).stroke(p.level(ring.level), style: StrokeStyle(lineWidth: stroke, lineCap: .round)) }
        }
        .frame(width: side - stroke, height: side - stroke)
    }
}
#endif
