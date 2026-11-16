#if os(macOS)
import AppKit
import SwiftUI

/// Kuvo's mark in the menu bar: the one place it's named, so the mark the user picks goes in here
/// alone. BRAND-NEEDED: a stand-in symbol until then (.claude/notes/761-drafts/logo-template.png).
public enum MenuBarMark {
    public static func image() -> NSImage {
        let image = NSImage(systemSymbolName: "circle.hexagonpath", accessibilityDescription: "Kuvo") ?? NSImage()
        let sized = image.withSymbolConfiguration(NSImage.SymbolConfiguration(pointSize: 14, weight: .regular)) ?? image
        sized.isTemplate = true
        return sized
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
    public var open: () -> Void
    public var stop: () -> Void

    public init(open: @escaping () -> Void, stop: @escaping () -> Void) {
        self.open = open
        self.stop = stop
    }
}

/// The item's panel (option A, picked 11-16): Kuvo on this Mac and where it is, what needs you,
/// what's at work (with the bot it works for), each account's rings, Open Kuvo, and Stop.
public struct MenuBarPanel: View {
    let look: MenuBarLook
    let actions: MenuBarActions
    @Environment(\.colorScheme) private var scheme

    /// The most rows of each kind it lists: the rest are counted ("2 more").
    static let most = 6

    public init(look: MenuBarLook, actions: MenuBarActions) {
        self.look = look
        self.actions = actions
    }

    public var body: some View {
        let p = Palette(dark: scheme == .dark)
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Kuvo on this Mac").font(.system(size: 13, weight: .semibold)).foregroundStyle(p.text)
                Spacer()
                HStack(spacing: 5) {
                    Circle().fill(look.stage == .stopping ? p.faint : (look.atWork == nil ? p.warning : p.success)).frame(width: 6, height: 6)
                    Text(look.status).font(.system(size: 12)).foregroundStyle(p.muted)
                }
                .accessibilityElement(children: .combine)
            }
            .padding(.horizontal, 10).frame(height: 30)
            Divider(p: p)
            if let atWork = look.atWork {
                content(atWork, p)
            }
            ActionRow(symbol: "macwindow", title: "Open Kuvo", shortcut: "⌘O", tint: p.text, p: p, action: actions.open)
                .keyboardShortcut("o", modifiers: .command)
            ActionRow(symbol: "stop.circle", title: "Stop Kuvo on this Mac…", shortcut: nil, tint: p.text, p: p, action: actions.stop)
                .disabled(!look.canStop)
                .opacity(look.canStop ? 1 : 0.5)
        }
        .padding(6)
        .frame(width: 300)
        .background(p.card)
    }

    @ViewBuilder
    private func content(_ atWork: AtWork, _ p: Palette) -> some View {
        if !atWork.asking.isEmpty {
            Heading(text: "Needs you", p: p)
            ForEach(atWork.asking.prefix(Self.most), id: \.id) { asking in
                Line(title: asking.title ?? "A session", detail: nil, p: p) { Circle().fill(p.warning).frame(width: 7, height: 7) }
                    .accessibilityLabel("\(asking.title ?? "A session"), \(asking.asks ?? "waits on you")")
                    .help(asking.asks ?? "")
            }
            More(count: max(atWork.waiting.sessions, atWork.asking.count) - min(atWork.asking.count, Self.most), p: p)
        }
        if !atWork.sessions.isEmpty {
            Heading(text: "At work", p: p)
            ForEach(atWork.sessions.prefix(Self.most), id: \.id) { session in
                Line(title: session.title ?? "A session", detail: session.bot, p: p) { Spinner(p: p) }
            }
            More(count: atWork.working - min(atWork.sessions.count, Self.most), p: p)
        }
        if atWork.asking.isEmpty, atWork.sessions.isEmpty {
            Text(atWork.background > 0 ? "Work is running in the background" : "Nothing at work")
                .font(.system(size: 12)).foregroundStyle(p.faint)
                .padding(.horizontal, 10).frame(height: 28)
        }
        if !atWork.rings.isEmpty {
            Divider(p: p)
            Accounts(accounts: atWork.rings, p: p)
        }
        Divider(p: p)
    }
}

private struct Divider: View {
    let p: Palette
    var body: some View { Rectangle().fill(p.border).frame(height: 1).padding(.horizontal, 10).padding(.vertical, 4) }
}

private struct Heading: View {
    let text: String
    let p: Palette
    var body: some View {
        Text(text).font(.system(size: 11, weight: .medium)).foregroundStyle(p.faint)
            .padding(.horizontal, 10).padding(.top, 6).padding(.bottom, 2)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityAddTraits(.isHeader)
    }
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
/// one inside, coloured by the box's word; its name, and the short window's share.
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
                            Text("\(Int((shown.used * 100).rounded()))%").font(.system(size: 11).monospacedDigit())
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
        let shares = [account.short, account.long].compactMap { $0 }.map { "\(Int(($0.used * 100).rounded())) percent" }
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
            Text(account.service.prefix(1).uppercased()).font(.system(size: rings.count > 1 ? 7 : 9, weight: .semibold)).foregroundStyle(p.muted)
        }
        .frame(width: size, height: size)
    }

    private func ring(_ ring: AtWork.Ring?, _ side: CGFloat) -> some View {
        ZStack {
            Circle().stroke(p.border, lineWidth: stroke)
            if let ring { Arc(used: ring.used).stroke(p.level(ring.level), style: StrokeStyle(lineWidth: stroke, lineCap: .round)) }
        }
        .frame(width: side - stroke, height: side - stroke)
    }
}
#endif
