// Drafts for #761: Kuvo Helper's menu bar item, its panel, and the desktop app's first-Quit note.
// Rendered headless with SwiftUI's ImageRenderer, light and dark: `swift render.swift <out folder>`.
// Made-up content only. Colours are the desktop app's (gpui/kuvo_ui/src/theme.rs).
import AppKit
import SwiftUI

// MARK: Palette (the desktop app's)

struct Palette {
    let dark: Bool
    func hex(_ v: UInt32, _ a: Double = 1) -> Color { Color(.sRGB, red: Double((v >> 16) & 255) / 255, green: Double((v >> 8) & 255) / 255, blue: Double(v & 255) / 255, opacity: a) }
    var background: Color { dark ? hex(0x0F0F0F) : hex(0xF3F3F3) }
    var card: Color { dark ? hex(0x1E1E1E) : hex(0xFFFFFF) }
    var elevated: Color { dark ? hex(0x242424) : hex(0xFFFFFF) }
    var sunken: Color { dark ? hex(0x121212) : hex(0xF4F4F4) }
    var text: Color { dark ? hex(0xECECEC) : hex(0x292929) }
    var muted: Color { dark ? hex(0xA3A3A3) : hex(0x5D5D5D) }
    var faint: Color { dark ? hex(0x6E6E6E) : hex(0x9E9E9E) }
    var icon: Color { dark ? hex(0x8F8F8F) : hex(0x7D7D7D) }
    var border: Color { dark ? hex(0x262626) : hex(0xEBEBEB) }
    var borderStrong: Color { dark ? hex(0x333333) : hex(0xDEDEDE) }
    var hover: Color { dark ? hex(0xFFFFFF, 0.05) : hex(0x292929, 0.04) }
    var primary: Color { dark ? hex(0xECECEC) : hex(0x292929) }
    var onPrimary: Color { dark ? hex(0x181818) : hex(0xFFFFFF) }
    var success: Color { dark ? hex(0x4CC27A) : hex(0x2E9E5B) }
    var warning: Color { dark ? hex(0xE0A43A) : hex(0xC7820E) }
    var danger: Color { dark ? hex(0xF06363) : hex(0xD93B3B) }
    var menuBar: Color { dark ? hex(0x1C1C1E, 0.92) : hex(0xF6F6F6, 0.92) }
    var menuGlyph: Color { dark ? .white : .black }
}

enum Level { case calm, near, over }
struct Ring { let used: Double; let level: Level }
struct Account { let mark: String; let name: String; let short: Ring; let long: Ring }
struct Working { let title: String; let bot: String? }

let working = [Working(title: "Fix sign-up form validation", bot: "Mira"), Working(title: "Nightly dependency bump", bot: nil), Working(title: "Write the release notes", bot: "Otto")]
let asking = [(title: "Deploy preview to staging", what: "Run a command")]
let accounts = [Account(mark: "C", name: "Claude", short: Ring(used: 0.62, level: .calm), long: Ring(used: 0.34, level: .calm)),
                Account(mark: "O", name: "Codex", short: Ring(used: 0.81, level: .near), long: Ring(used: 0.22, level: .calm))]

func levelColor(_ l: Level, _ p: Palette) -> Color { switch l { case .calm: p.success; case .near: p.warning; case .over: p.danger } }

// MARK: Pieces

struct Arc: Shape {
    var used: Double
    func path(in rect: CGRect) -> Path {
        var p = Path()
        p.addArc(center: CGPoint(x: rect.midX, y: rect.midY), radius: rect.width / 2, startAngle: .degrees(-90), endAngle: .degrees(-90 + 360 * used), clockwise: false)
        return p
    }
}

/// An account's two rings (short outside, long inside), as the rail draws them.
struct Rings: View {
    let account: Account
    let size: CGFloat
    let stroke: CGFloat
    let p: Palette
    var tinted = true
    var body: some View {
        ZStack {
            ring(account.short, size)
            ring(account.long, size - (stroke + 1.5) * 2)
            Text(account.mark).font(.system(size: size * 0.3, weight: .semibold)).foregroundStyle(p.muted)
        }.frame(width: size, height: size)
    }
    func ring(_ r: Ring, _ s: CGFloat) -> some View {
        ZStack {
            Circle().stroke(p.border, lineWidth: stroke).frame(width: s - stroke, height: s - stroke)
            Arc(used: r.used).stroke(tinted ? levelColor(r.level, p) : p.menuGlyph, style: StrokeStyle(lineWidth: stroke, lineCap: .round)).frame(width: s - stroke, height: s - stroke)
        }
    }
}

struct Spinner: View {
    let p: Palette
    var body: some View {
        ZStack {
            Circle().stroke(p.border, lineWidth: 1.5)
            Arc(used: 0.28).stroke(p.icon, style: StrokeStyle(lineWidth: 1.5, lineCap: .round))
        }.frame(width: 11, height: 11)
    }
}

struct Row<Lead: View>: View {
    let lead: Lead
    let title: String
    let detail: String?
    let p: Palette
    var body: some View {
        HStack(spacing: 9) {
            lead.frame(width: 14)
            Text(title).font(.system(size: 13)).foregroundStyle(p.text).lineLimit(1)
            Spacer(minLength: 8)
            if let detail { Text(detail).font(.system(size: 12)).foregroundStyle(p.faint) }
        }
        .padding(.horizontal, 10).frame(height: 28)
    }
}

struct SectionTitle: View {
    let text: String
    let p: Palette
    var body: some View {
        Text(text).font(.system(size: 11, weight: .medium)).foregroundStyle(p.faint).padding(.horizontal, 10).padding(.top, 6).padding(.bottom, 2).frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct ActionRow: View {
    let symbol: String
    let title: String
    let shortcut: String?
    let p: Palette
    var danger = false
    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: symbol).font(.system(size: 12, weight: .light)).foregroundStyle(danger ? p.danger : p.icon).frame(width: 14)
            Text(title).font(.system(size: 13)).foregroundStyle(danger ? p.danger : p.text)
            Spacer()
            if let shortcut { Text(shortcut).font(.system(size: 12)).foregroundStyle(p.faint) }
        }.padding(.horizontal, 10).frame(height: 28)
    }
}

struct Divide: View {
    let p: Palette
    var body: some View { Rectangle().fill(p.border).frame(height: 1).padding(.horizontal, 10).padding(.vertical, 4) }
}

// MARK: The menu bar strip, with the item among a few of the system's

enum Glyph { case mark(count: Int?, dot: Bool), rings }

struct GlyphView: View {
    let glyph: Glyph
    let p: Palette
    var body: some View {
        switch glyph {
        case .mark(let count, let dot):
            HStack(spacing: 3) {
                ZStack(alignment: .topTrailing) {
                    // A stand-in for Kuvo's menu bar mark (a brand decision).
                    Image(systemName: "circle.hexagonpath").font(.system(size: 14, weight: .regular)).foregroundStyle(p.menuGlyph)
                    if dot { Circle().fill(p.warning).frame(width: 6, height: 6).offset(x: 2, y: -1) }
                }
                if let count { Text("\(count)").font(.system(size: 12, weight: .medium).monospacedDigit()).foregroundStyle(p.menuGlyph) }
            }
        case .rings:
            HStack(spacing: 4) {
                ForEach(accounts.indices, id: \.self) { i in
                    ZStack {
                        Circle().stroke(p.menuGlyph.opacity(0.25), lineWidth: 1.6).frame(width: 13, height: 13)
                        Arc(used: accounts[i].short.used).stroke(p.menuGlyph, style: StrokeStyle(lineWidth: 1.6, lineCap: .round)).frame(width: 13, height: 13)
                        Circle().stroke(p.menuGlyph.opacity(0.25), lineWidth: 1.4).frame(width: 7, height: 7)
                        Arc(used: accounts[i].long.used).stroke(p.menuGlyph, style: StrokeStyle(lineWidth: 1.4, lineCap: .round)).frame(width: 7, height: 7)
                    }
                }
                Circle().fill(p.warning).frame(width: 5, height: 5)
            }
        }
    }
}

struct MenuBar: View {
    let glyph: Glyph
    let p: Palette
    var open = true
    var body: some View {
        HStack(spacing: 14) {
            Spacer()
            GlyphView(glyph: glyph, p: p).padding(.horizontal, 6).padding(.vertical, 2).background(RoundedRectangle(cornerRadius: 4).fill(open ? p.menuGlyph.opacity(0.12) : .clear))
            Image(systemName: "wifi").font(.system(size: 13)).foregroundStyle(p.menuGlyph)
            Image(systemName: "battery.75percent").font(.system(size: 14)).foregroundStyle(p.menuGlyph)
            Image(systemName: "switch.2").font(.system(size: 12)).foregroundStyle(p.menuGlyph)
            Text("Mon 16 Nov  9:41").font(.system(size: 13, weight: .medium)).foregroundStyle(p.menuGlyph)
        }
        .padding(.horizontal, 14).frame(width: 520, height: 24).background(p.menuBar)
    }
}

// MARK: Panels

/// A: a card of the desktop app's own look: what's at work, what needs you, the rings, two actions.
struct PanelA: View {
    let p: Palette
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Kuvo on this Mac").font(.system(size: 13, weight: .semibold)).foregroundStyle(p.text)
                Spacer()
                HStack(spacing: 5) { Circle().fill(p.success).frame(width: 6, height: 6); Text("Running").font(.system(size: 12)).foregroundStyle(p.muted) }
            }.padding(.horizontal, 10).frame(height: 30)
            Divide(p: p)
            SectionTitle(text: "Needs you", p: p)
            ForEach(asking.indices, id: \.self) { i in Row(lead: Circle().fill(p.warning).frame(width: 7, height: 7), title: asking[i].title, detail: nil, p: p) }
            SectionTitle(text: "At work", p: p)
            ForEach(working.indices, id: \.self) { i in Row(lead: Spinner(p: p), title: working[i].title, detail: working[i].bot, p: p) }
            Divide(p: p)
            HStack(spacing: 14) {
                ForEach(accounts.indices, id: \.self) { i in
                    HStack(spacing: 7) {
                        Rings(account: accounts[i], size: 26, stroke: 2.5, p: p)
                        VStack(alignment: .leading, spacing: 0) {
                            Text(accounts[i].name).font(.system(size: 12, weight: .medium)).foregroundStyle(p.text)
                            Text("\(Int(accounts[i].short.used * 100))%").font(.system(size: 11).monospacedDigit()).foregroundStyle(accounts[i].short.level == .calm ? p.faint : p.warning)
                        }
                    }
                }
                Spacer()
            }.padding(.horizontal, 10).padding(.vertical, 6)
            Divide(p: p)
            ActionRow(symbol: "macwindow", title: "Open Kuvo", shortcut: "⌘O", p: p)
            ActionRow(symbol: "stop.circle", title: "Stop Kuvo on this Mac…", shortcut: nil, p: p)
        }
        .padding(6).frame(width: 300)
        .background(RoundedRectangle(cornerRadius: 12).fill(p.card))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(p.borderStrong, lineWidth: 1))
        .shadow(color: .black.opacity(p.dark ? 0.45 : 0.12), radius: 14, y: 6)
    }
}

/// B: a plain macOS menu (Tailscale's, Docker's): the same facts as menu items.
struct PanelB: View {
    let p: Palette
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Kuvo on this Mac is running").font(.system(size: 13)).foregroundStyle(p.faint).padding(.horizontal, 12).frame(height: 22)
            Rectangle().fill(p.borderStrong).frame(height: 1).padding(.horizontal, 12).padding(.vertical, 5)
            Text("Needs you").font(.system(size: 11, weight: .semibold)).foregroundStyle(p.faint).padding(.horizontal, 12).frame(height: 20)
            ForEach(asking.indices, id: \.self) { i in menuItem(asking[i].title, lead: AnyView(Circle().fill(p.warning).frame(width: 7, height: 7))) }
            Text("At work").font(.system(size: 11, weight: .semibold)).foregroundStyle(p.faint).padding(.horizontal, 12).frame(height: 20)
            ForEach(working.indices, id: \.self) { i in menuItem(working[i].title + (working[i].bot.map { " — \($0)" } ?? ""), lead: AnyView(Spinner(p: p))) }
            Rectangle().fill(p.borderStrong).frame(height: 1).padding(.horizontal, 12).padding(.vertical, 5)
            ForEach(accounts.indices, id: \.self) { i in
                menuItem("\(accounts[i].name)   \(Int(accounts[i].short.used * 100))% · week \(Int(accounts[i].long.used * 100))%", lead: AnyView(Rings(account: accounts[i], size: 16, stroke: 1.6, p: p)))
            }
            Rectangle().fill(p.borderStrong).frame(height: 1).padding(.horizontal, 12).padding(.vertical, 5)
            menuItem("Open Kuvo", lead: AnyView(EmptyView()), shortcut: "⌘O")
            menuItem("Stop Kuvo on this Mac…", lead: AnyView(EmptyView()))
        }
        .padding(.vertical, 5).frame(width: 290)
        .background(RoundedRectangle(cornerRadius: 9).fill(p.dark ? p.hex(0x2A2A2C) : p.hex(0xF0F0F0)))
        .overlay(RoundedRectangle(cornerRadius: 9).stroke(p.borderStrong, lineWidth: 0.5))
        .shadow(color: .black.opacity(0.25), radius: 12, y: 6)
    }
    func menuItem(_ title: String, lead: AnyView, shortcut: String? = nil) -> some View {
        HStack(spacing: 7) {
            lead.frame(width: 16)
            Text(title).font(.system(size: 13)).foregroundStyle(p.text).lineLimit(1)
            Spacer()
            if let shortcut { Text(shortcut).font(.system(size: 13)).foregroundStyle(p.faint) }
        }.padding(.horizontal, 12).frame(height: 22)
    }
}

/// C: a glance: counts and the rings, large; no list (sessions are in the app).
struct PanelC: View {
    let p: Palette
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Kuvo on this Mac").font(.system(size: 13, weight: .semibold)).foregroundStyle(p.text)
                Spacer()
                HStack(spacing: 5) { Circle().fill(p.success).frame(width: 6, height: 6); Text("Running").font(.system(size: 12)).foregroundStyle(p.muted) }
            }
            HStack(spacing: 8) {
                tile(symbol: "circle.dotted", value: "\(working.count)", label: "At work")
                tile(symbol: "exclamationmark.bubble", value: "\(asking.count)", label: "Needs you", warn: true)
                tile(symbol: "alarm", value: "2", label: "Wake-ups")
            }
            HStack(spacing: 18) {
                ForEach(accounts.indices, id: \.self) { i in
                    VStack(spacing: 4) {
                        Rings(account: accounts[i], size: 40, stroke: 3.5, p: p)
                        Text(accounts[i].name).font(.system(size: 11)).foregroundStyle(p.muted)
                    }
                }
            }.frame(maxWidth: .infinity)
            HStack(spacing: 8) {
                Text("Open Kuvo").font(.system(size: 13, weight: .medium)).foregroundStyle(p.onPrimary).frame(maxWidth: .infinity).frame(height: 28).background(RoundedRectangle(cornerRadius: 7).fill(p.primary))
                Text("Stop…").font(.system(size: 13)).foregroundStyle(p.text).frame(width: 70, height: 28).background(RoundedRectangle(cornerRadius: 7).stroke(p.borderStrong))
            }
        }
        .padding(14).frame(width: 290)
        .background(RoundedRectangle(cornerRadius: 12).fill(p.card))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(p.borderStrong, lineWidth: 1))
        .shadow(color: .black.opacity(p.dark ? 0.45 : 0.12), radius: 14, y: 6)
    }
    func tile(symbol: String, value: String, label: String, warn: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                Image(systemName: symbol).font(.system(size: 11, weight: .light)).foregroundStyle(warn ? p.warning : p.icon)
                Text(value).font(.system(size: 15, weight: .semibold).monospacedDigit()).foregroundStyle(p.text)
            }
            Text(label).font(.system(size: 11)).foregroundStyle(p.faint)
        }.padding(8).frame(maxWidth: .infinity, alignment: .leading).background(RoundedRectangle(cornerRadius: 8).fill(p.sunken))
    }
}

/// The stop confirmation (an alert from the item, while something's at work).
struct StopConfirm: View {
    let p: Palette
    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "stop.circle").font(.system(size: 30, weight: .light)).foregroundStyle(p.icon)
            Text("Stop Kuvo on this Mac?").font(.system(size: 13, weight: .semibold)).foregroundStyle(p.text)
            Text("3 sessions are at work. They stop now, and Kuvo starts again when you open it or log in.").font(.system(size: 11)).foregroundStyle(p.muted).multilineTextAlignment(.center).fixedSize(horizontal: false, vertical: true)
            VStack(spacing: 6) {
                Text("Stop").font(.system(size: 13, weight: .medium)).foregroundStyle(.white).frame(maxWidth: .infinity).frame(height: 26).background(RoundedRectangle(cornerRadius: 7).fill(p.danger))
                Text("Cancel").font(.system(size: 13)).foregroundStyle(p.text).frame(maxWidth: .infinity).frame(height: 26).background(RoundedRectangle(cornerRadius: 7).fill(p.sunken))
            }.padding(.top, 4)
        }
        .padding(16).frame(width: 260)
        .background(RoundedRectangle(cornerRadius: 14).fill(p.elevated))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(p.borderStrong, lineWidth: 1))
    }
}

// MARK: Quit notes

/// 1: in the desktop app's window, as ⌘Q is pressed the first time: a card of its own look.
struct QuitNoteWindow: View {
    let p: Palette
    var body: some View {
        ZStack {
            // A dimmed window behind it.
            RoundedRectangle(cornerRadius: 10).fill(p.background).overlay(
                HStack(spacing: 0) {
                    Rectangle().fill(p.dark ? p.hex(0x141414) : p.hex(0xF8F8F8)).frame(width: 190)
                    Rectangle().fill(p.dark ? p.hex(0x181818) : .white)
                }.clipShape(RoundedRectangle(cornerRadius: 10))
            ).overlay(Color.black.opacity(p.dark ? 0.45 : 0.18).clipShape(RoundedRectangle(cornerRadius: 10)))
            VStack(alignment: .leading, spacing: 12) {
                MenuBar(glyph: .mark(count: 3, dot: true), p: p, open: true).frame(width: 300).clipShape(RoundedRectangle(cornerRadius: 6)).overlay(RoundedRectangle(cornerRadius: 6).stroke(p.border))
                Text("Kuvo keeps working").font(.system(size: 15, weight: .semibold)).foregroundStyle(p.text)
                Text("Your sessions, bots and wake-ups go on after you quit. Kuvo is in the menu bar: open it or stop it there.").font(.system(size: 13)).foregroundStyle(p.muted).fixedSize(horizontal: false, vertical: true)
                HStack {
                    Spacer()
                    Text("Cancel").font(.system(size: 13)).foregroundStyle(p.text).padding(.horizontal, 14).frame(height: 28).background(RoundedRectangle(cornerRadius: 7).stroke(p.borderStrong))
                    Text("Quit").font(.system(size: 13, weight: .medium)).foregroundStyle(p.onPrimary).padding(.horizontal, 18).frame(height: 28).background(RoundedRectangle(cornerRadius: 7).fill(p.primary))
                }
            }
            .padding(18).frame(width: 340)
            .background(RoundedRectangle(cornerRadius: 12).fill(p.card))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(p.borderStrong, lineWidth: 1))
            .shadow(color: .black.opacity(0.25), radius: 20, y: 8)
        }.frame(width: 620, height: 380)
    }
}

/// 2: after quitting, the item's own panel opens once, with a note on top.
struct QuitNoteFromItem: View {
    let p: Palette
    var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            MenuBar(glyph: .mark(count: 3, dot: true), p: p)
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .top, spacing: 9) {
                    Image(systemName: "arrow.up.forward.circle").font(.system(size: 15, weight: .light)).foregroundStyle(p.icon)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Kuvo keeps working").font(.system(size: 13, weight: .semibold)).foregroundStyle(p.text)
                        Text("You quit the window; your sessions go on. Kuvo stays here.").font(.system(size: 12)).foregroundStyle(p.muted).fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: "xmark").font(.system(size: 10)).foregroundStyle(p.faint)
                }.padding(10).background(RoundedRectangle(cornerRadius: 8).fill(p.sunken))
                PanelA(p: p).shadow(radius: 0)
            }
            .padding(.trailing, 150)
        }.frame(width: 520)
    }
}

// MARK: Sheets

@MainActor
func sheet<V: View>(_ title: String, _ content: @escaping (Palette) -> V) -> some View {
    HStack(alignment: .top, spacing: 0) {
        ForEach([false, true], id: \.self) { dark in
            let p = Palette(dark: dark)
            VStack(alignment: .leading, spacing: 10) {
                Text(dark ? "Dark" : "Light").font(.system(size: 11, weight: .medium)).foregroundStyle(p.faint)
                content(p)
            }
            .padding(24)
            .background(dark ? Color(white: 0.16) : Color(white: 0.86))
            .environment(\.colorScheme, dark ? .dark : .light)
        }
    }
}

@MainActor
func optionSheet(_ glyph: Glyph, _ panel: @escaping (Palette) -> AnyView) -> some View {
    sheet("") { p in
        VStack(alignment: .trailing, spacing: 6) {
            MenuBar(glyph: glyph, p: p)
            panel(p).padding(.trailing, 150)
        }
    }
}

@MainActor
func write(_ view: some View, _ name: String, to folder: URL) {
    let renderer = ImageRenderer(content: view)
    renderer.scale = 2
    guard let image = renderer.nsImage, let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff), let png = rep.representation(using: .png, properties: [:]) else { fatalError("render \(name)") }
    try! png.write(to: folder.appending(path: name))
    print(name)
}

let out = URL(filePath: CommandLine.arguments.dropFirst().first ?? ".")
MainActor.assumeIsolated {
    write(optionSheet(.mark(count: 3, dot: true)) { AnyView(PanelA(p: $0)) }, "A-card.png", to: out)
    write(optionSheet(.mark(count: nil, dot: true)) { AnyView(PanelB(p: $0)) }, "B-menu.png", to: out)
    write(optionSheet(.rings) { AnyView(PanelC(p: $0)) }, "C-glance.png", to: out)
    write(sheet("") { p in VStack(alignment: .leading, spacing: 8) {
        MenuBar(glyph: .mark(count: 3, dot: true), p: p, open: false)
        MenuBar(glyph: .mark(count: 3, dot: false), p: p, open: false)
        MenuBar(glyph: .mark(count: nil, dot: false), p: p, open: false)
        MenuBar(glyph: .rings, p: p, open: false)
    } }, "item-states.png", to: out)
    write(sheet("") { StopConfirm(p: $0) }, "stop-confirm.png", to: out)
    write(sheet("") { QuitNoteWindow(p: $0) }, "quit-1-in-window.png", to: out)
    write(sheet("") { QuitNoteFromItem(p: $0) }, "quit-2-from-item.png", to: out)
}
