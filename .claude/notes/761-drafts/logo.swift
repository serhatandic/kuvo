// #761: Kuvo's logo as a one-colour menu bar template, beside the stand-in, for the user to pick.
// `swiftc -O -o logo logo.swift && ./logo <repo> <out folder>`.
//
// The logo is the app icon's own artwork (swift/KuvoCode/AppIcon.icon/Assets/artwork-light.png),
// unchanged in shape: its coloured shapes become the one colour and everything else (the white
// ground, the eyes) becomes clear. In "with gaps", a thin clear line is left where two shapes'
// colours meet, as the colours told them apart. The website's favicon (kuvo-web dist/favicon.svg) is
// another existing mark, so it's shown too, its own path and dot made one colour.
import AppKit
import SwiftUI

let arguments = CommandLine.arguments
let repo = URL(filePath: arguments.count > 1 ? arguments[1] : ".")
let out = URL(filePath: arguments.count > 2 ? arguments[2] : ".")

/// The artwork as a one-colour template: ink where it's coloured, clear elsewhere; `gap` pixels of
/// clear between shapes of different colours.
func template(_ file: URL, gap: Int) -> NSImage {
    let source = NSImage(contentsOf: file)!.cgImage(forProposedRect: nil, context: nil, hints: nil)!
    let w = source.width, h = source.height
    var pixels = [UInt8](repeating: 0, count: w * h * 4)
    let space = CGColorSpaceCreateDeviceRGB()
    let context = CGContext(data: &pixels, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.draw(source, in: CGRect(x: 0, y: 0, width: w, height: h))
    var hue = [Float](repeating: -1, count: w * h)
    for i in 0..<(w * h) {
        let r = Float(pixels[i * 4]) / 255, g = Float(pixels[i * 4 + 1]) / 255, b = Float(pixels[i * 4 + 2]) / 255, a = Float(pixels[i * 4 + 3]) / 255
        let mx = max(r, g, b), mn = min(r, g, b)
        guard a > 0.5, mx > 0.2, (mx - mn) / mx > 0.3 else { continue }
        var hh: Float
        if mx == r { hh = (g - b) / (mx - mn) } else if mx == g { hh = 2 + (b - r) / (mx - mn) } else { hh = 4 + (r - g) / (mx - mn) }
        hh *= 60; if hh < 0 { hh += 360 }
        hue[i] = hh
    }
    func differs(_ a: Float, _ b: Float) -> Bool { let d = abs(a - b); return min(d, 360 - d) > 20 }
    var ink = [UInt8](repeating: 0, count: w * h * 4)
    for y in 0..<h {
        for x in 0..<w {
            let i = y * w + x
            guard hue[i] >= 0 else { continue }
            var edge = false
            if gap > 0 {
                search: for dy in stride(from: -gap, through: gap, by: 2) {
                    for dx in stride(from: -gap, through: gap, by: 2) where dx * dx + dy * dy <= gap * gap {
                        let xx = x + dx, yy = y + dy
                        guard xx >= 0, yy >= 0, xx < w, yy < h else { continue }
                        let other = hue[yy * w + xx]
                        if other >= 0, differs(other, hue[i]) { edge = true; break search }
                    }
                }
            }
            if !edge { ink[i * 4 + 3] = 255 }
        }
    }
    let made = CGContext(data: &ink, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!.makeImage()!
    let image = NSImage(cgImage: made, size: NSSize(width: w, height: h))
    image.isTemplate = true
    return image
}

/// The website's favicon's mark (its hexagon and dot, without its tile), one colour.
struct Hexagon: View {
    var body: some View {
        Canvas { context, size in
            let s = size.width / 34
            context.translateBy(x: -15 * s, y: -15 * s)
            var p = Path()
            p.move(to: CGPoint(x: 32 * s, y: 15.75 * s))
            for (x, y) in [(46.06, 23.88), (46.06, 40.13), (32, 48.25), (17.94, 40.13), (17.94, 23.88)] { p.addLine(to: CGPoint(x: x * s, y: y * s)) }
            p.closeSubpath()
            context.stroke(p, with: .foreground, style: StrokeStyle(lineWidth: 6.5 * s, lineJoin: .round))
            context.fill(Path(ellipseIn: CGRect(x: (32 - 4.7) * s, y: (32 - 4.7) * s, width: 9.4 * s, height: 9.4 * s)), with: .foreground)
        }
    }
}

let artwork = repo.appending(path: "swift/KuvoCode/AppIcon.icon/Assets/artwork-light.png")
let withGaps = template(artwork, gap: 22)
let solid = template(artwork, gap: 0)

struct Candidate {
    let name: String
    let glyph: AnyView
}

func bar(_ glyph: AnyView, dark: Bool, count: String?) -> some View {
    let ink: Color = dark ? .white : .black
    return HStack(spacing: 14) {
        Spacer()
        HStack(spacing: 3) {
            glyph.foregroundStyle(ink)
            if let count { Text(count).font(.system(size: 12, weight: .medium).monospacedDigit()).foregroundStyle(ink) }
        }
        Image(systemName: "wifi").font(.system(size: 13)).foregroundStyle(ink)
        Image(systemName: "battery.75percent").font(.system(size: 14)).foregroundStyle(ink)
        Text("Mon 16 Nov  9:41").font(.system(size: 13, weight: .medium)).foregroundStyle(ink)
    }
    .padding(.horizontal, 12).frame(width: 330, height: 24).background(dark ? Color(white: 0.11) : Color(white: 0.965))
}

let candidates: [Candidate] = [
    Candidate(name: "Stand-in (SF Symbol)", glyph: AnyView(Image(systemName: "circle.hexagonpath").font(.system(size: 14)))),
    Candidate(name: "App icon artwork, with gaps", glyph: AnyView(Image(nsImage: withGaps).renderingMode(.template).resizable().interpolation(.high).frame(width: 17, height: 17))),
    Candidate(name: "App icon artwork, solid", glyph: AnyView(Image(nsImage: solid).renderingMode(.template).resizable().interpolation(.high).frame(width: 17, height: 17))),
    Candidate(name: "Website favicon's mark", glyph: AnyView(Hexagon().frame(width: 15, height: 15))),
]

let sheet = VStack(alignment: .leading, spacing: 14) {
    ForEach(candidates.indices, id: \.self) { i in
        VStack(alignment: .leading, spacing: 5) {
            Text(candidates[i].name).font(.system(size: 12, weight: .medium)).foregroundStyle(Color(white: 0.3))
            HStack(spacing: 16) {
                bar(candidates[i].glyph, dark: false, count: "3")
                bar(candidates[i].glyph, dark: true, count: "3")
            }
            HStack(spacing: 16) {
                bar(candidates[i].glyph, dark: false, count: nil)
                bar(candidates[i].glyph, dark: true, count: nil)
            }
        }
    }
    // Large, to see the shapes.
    HStack(spacing: 24) {
        ForEach(candidates.indices, id: \.self) { i in
            VStack(spacing: 4) {
                candidates[i].glyph.scaleEffect(4).frame(width: 80, height: 80).foregroundStyle(.black)
                Text(candidates[i].name).font(.system(size: 10)).foregroundStyle(Color(white: 0.35)).frame(width: 140)
            }
        }
    }.padding(.top, 10)
}
.padding(24).background(Color(white: 0.88))

MainActor.assumeIsolated {
    let renderer = ImageRenderer(content: sheet)
    renderer.scale = 3
    let image = renderer.nsImage!
    let png = NSBitmapImageRep(data: image.tiffRepresentation!)!.representation(using: .png, properties: [:])!
    try! png.write(to: out.appending(path: "logo-template.png"))
    // The template itself, for whoever picks it.
    for (name, made) in [("logo-template-gaps", withGaps), ("logo-template-solid", solid)] {
        let small = NSImage(size: NSSize(width: 64, height: 64), flipped: false) { rect in made.draw(in: rect); return true }
        let data = NSBitmapImageRep(data: small.tiffRepresentation!)!.representation(using: .png, properties: [:])!
        try! data.write(to: out.appending(path: "\(name).png"))
    }
    // The picked mark (11-16, "with gaps") at the sizes it's used: Kuvo Helper's menu bar item
    // (a template, 18 pt at 3x) and the desktop app's first-Quit card (black and white, 48 px).
    func tinted(_ image: NSImage, _ side: Int, _ color: NSColor) -> Data {
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSGraphicsContext.current?.imageInterpolation = .high
        let rect = NSRect(x: 0, y: 0, width: side, height: side)
        image.draw(in: rect)
        color.set()
        rect.fill(using: .sourceIn)
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])!
    }
    try! tinted(withGaps, 54, .black).write(to: out.appending(path: "kuvo-mark-template-54.png"))
    try! tinted(withGaps, 48, .black).write(to: out.appending(path: "kuvo-mark-black-48.png"))
    try! tinted(withGaps, 48, .white).write(to: out.appending(path: "kuvo-mark-white-48.png"))
    print("logo-template.png")
}
