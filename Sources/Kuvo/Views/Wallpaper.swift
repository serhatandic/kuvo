import AppKit
import SwiftUI

/// How bright the desktop picture is, 0 (dark) to 1 (light). Light-mode glass
/// uses it: over a light wallpaper the window can be properly translucent;
/// over a dark one, translucency just turns light mode gray.
@MainActor @Observable
final class Wallpaper {
    static let shared = Wallpaper()
    private(set) var luminance = 0.0 // Assume dark until measured: the safe default.

    private init() {
        measure()
        let center = NSWorkspace.shared.notificationCenter
        center.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { Wallpaper.shared.measure() }
        }
        NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { Wallpaper.shared.measure() }
        }
        DistributedNotificationCenter.default().addObserver(forName: .init("AppleInterfaceThemeChangedNotification"), object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { Wallpaper.shared.measure() }
        }
    }

    func measure() {
        guard let screen = NSScreen.main,
              let url = NSWorkspace.shared.desktopImageURL(for: screen),
              let image = NSImage(contentsOf: url),
              let cgImage = image.cgImage(forProposedRect: nil, context: nil, hints: nil)
        else { return }
        // Average color by drawing the whole picture into a single pixel.
        var pixel = [UInt8](repeating: 0, count: 4)
        guard let context = CGContext(data: &pixel, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 4,
                                      space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return }
        context.interpolationQuality = .medium
        context.draw(cgImage, in: CGRect(x: 0, y: 0, width: 1, height: 1))
        var value = (0.2126 * Double(pixel[0]) + 0.7152 * Double(pixel[1]) + 0.0722 * Double(pixel[2])) / 255
        // Dynamic wallpapers show their dark variant when macOS itself is dark, whatever
        // Kuvo's own appearance is; the image above is the light variant.
        if UserDefaults.standard.string(forKey: "AppleInterfaceStyle") == "Dark" { value = min(value, 0.15) }
        if abs(value - luminance) > 0.02 { withAnimation(.smooth) { luminance = value } }
    }
}
