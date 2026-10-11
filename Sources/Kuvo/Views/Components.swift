import SwiftUI

// MARK: - Status

extension Container.State {
    var color: Color {
        switch self {
        case .running: .green
        case .paused: .yellow
        case .restarting, .removing: .orange
        case .created: .blue
        case .exited, .dead: .secondary
        }
    }

    var label: String {
        switch self {
        case .running: "Running"
        case .paused: "Paused"
        case .restarting: "Restarting"
        case .removing: "Removing"
        case .created: "Created"
        case .exited: "Stopped"
        case .dead: "Dead"
        }
    }
}

extension ContainerAction {
    var progressLabel: String {
        switch self {
        case .start: "Starting"
        case .stop: "Stopping"
        case .restart: "Restarting"
        case .pause: "Pausing"
        case .unpause: "Resuming"
        case .remove: "Removing"
        }
    }
}

/// A dot that breathes while the container is running.
struct StatusDot: View {
    let state: Container.State
    var busy = false
    var size: CGFloat = 8

    var body: some View {
        ZStack {
            if state == .running && !busy {
                BreathingHalo(color: .systemGreen)
            }
            Circle()
                .fill(busy ? Color.orange : state.color)
                .opacity(busy ? 0.9 : 1)
                .scaleEffect(busy ? 0.75 : 1)
                .phaseAnimator([false, true], trigger: busy) { dot, phase in
                    dot.opacity(busy && phase ? 0.35 : 1)
                } animation: { _ in busy ? .easeInOut(duration: 0.5).repeatForever() : .default }
        }
        .frame(width: size, height: size)
        .animation(.smooth, value: state)
        .animation(.smooth, value: busy)
    }
}

/// The running dot's halo, animated by Core Animation in the render server. As a
/// SwiftUI animation it kept the display cycle running forever, which re-laid out
/// the whole window on every frame (about a quarter of a core with the window open).
private struct BreathingHalo: NSViewRepresentable {
    let color: NSColor

    func makeNSView(context: Context) -> HaloView { HaloView(color: color) }
    func updateNSView(_ view: HaloView, context: Context) {}

    final class HaloView: NSView {
        private let halo = CALayer()

        init(color: NSColor) {
            super.init(frame: .zero)
            wantsLayer = true
            clipsToBounds = false
            halo.backgroundColor = color.withAlphaComponent(0.35).cgColor
            halo.opacity = 0
            layer?.addSublayer(halo)
        }

        /// Core Animation drops animations when a layer leaves the window, so
        /// (re)start the breath whenever the view lands in one.
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard window != nil, halo.animation(forKey: "breath") == nil else { return }
            let grow = CABasicAnimation(keyPath: "transform.scale")
            grow.fromValue = 1
            grow.toValue = 2.2
            let fade = CABasicAnimation(keyPath: "opacity")
            fade.fromValue = 0.9
            fade.toValue = 0
            let breath = CAAnimationGroup()
            breath.animations = [grow, fade]
            breath.duration = 1.8
            breath.timingFunction = CAMediaTimingFunction(name: .easeOut)
            breath.repeatCount = .infinity
            breath.isRemovedOnCompletion = false
            halo.add(breath, forKey: "breath")
        }

        required init?(coder: NSCoder) { fatalError() }

        override func layout() {
            super.layout()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            halo.frame = bounds
            halo.cornerRadius = bounds.width / 2
            CATransaction.commit()
        }
    }
}

// MARK: - Sparkline

/// A live graph that scrolls one step left each time a sample arrives. Drawn and
/// animated with Core Animation: each sample builds one path, and the glide plays in
/// the render server. Frame-by-frame SwiftUI animation here made SwiftUI update and
/// lay out the whole window on every frame, costing about a fifth of a core.
struct Sparkline: NSViewRepresentable {
    let values: [Double]
    let tick: Int
    var minimum: Double = 1
    var color: Color = .accentColor
    var lineWidth: CGFloat = 1.5
    /// Levels like memory read better padded with their first sample; rates with zero.
    var isLevel = false

    func makeNSView(context: Context) -> SparklineView { SparklineView() }

    func updateNSView(_ view: SparklineView, context: Context) {
        // Pad to a full window so a fresh series reads as a flat line with data flowing in.
        let padded = Array(repeating: isLevel ? values.first ?? 0 : 0, count: max(LiveStats.window + 1 - values.count, 0)) + values
        // Rounded up so the scale doesn't twitch with every sample.
        let peak = max(values.max() ?? 0, minimum) * 1.15
        let magnitude = pow(10, log10(peak).rounded(.down))
        let ceiling = (peak / magnitude * 2).rounded(.up) / 2 * magnitude
        let light = context.environment.colorScheme == .light
        // Lighter on white cards, where color reads heavier than on dark glass.
        view.update(values: padded, tick: tick, ceiling: ceiling, color: NSColor(color),
                    fillOpacity: light ? 0.18 : 0.28, lineWidth: light ? lineWidth * 0.85 : lineWidth)
    }
}

final class SparklineView: NSView {
    private let strip = CALayer()
    private let line = CAShapeLayer()
    private let fill = CAGradientLayer()
    private let fillMask = CAShapeLayer()
    private var input: (values: [Double], tick: Int, ceiling: Double)?
    private var drawn: (tick: Int, size: CGSize)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = true
        line.fillColor = nil
        line.lineCap = .round
        line.lineJoin = .round
        fill.mask = fillMask
        fill.startPoint = CGPoint(x: 0.5, y: 1) // Top, in AppKit's bottom-up layer space.
        fill.endPoint = CGPoint(x: 0.5, y: 0)
        strip.addSublayer(fill)
        strip.addSublayer(line)
        layer?.addSublayer(strip)
    }

    required init?(coder: NSCoder) { fatalError() }

    func update(values: [Double], tick: Int, ceiling: Double, color: NSColor, fillOpacity: Double, lineWidth: CGFloat) {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            line.strokeColor = color.cgColor
            fill.colors = [color.withAlphaComponent(fillOpacity).cgColor, color.withAlphaComponent(0).cgColor]
        }
        line.lineWidth = lineWidth
        input = (values, tick, ceiling)
        redraw()
    }

    override func layout() {
        super.layout()
        redraw()
    }

    /// Rebuilds the path when a sample arrives (or the size changes) and glides it one step left.
    private func redraw() {
        guard let input, bounds.width > 0, bounds.height > 0 else { return }
        let size = bounds.size
        guard drawn?.tick != input.tick || drawn?.size != size else { return }
        let advanced = drawn.map { input.tick > $0.tick && $0.size == size } ?? false
        drawn = (input.tick, size)

        let step = size.width / CGFloat(LiveStats.window - 1)
        let inset: CGFloat = 1.5
        let ceiling = max(input.ceiling, .leastNonzeroMagnitude)
        let points = input.values.enumerated().map { i, v in
            CGPoint(x: size.width - CGFloat(input.values.count - 1 - i) * step,
                    y: inset + CGFloat(min(v / ceiling, 1)) * (size.height - inset * 2))
        }
        let path = CGMutablePath()
        path.move(to: points[0])
        for (a, b) in zip(points, points.dropFirst()) {
            let mid = (a.x + b.x) / 2
            path.addCurve(to: b, control1: CGPoint(x: mid, y: a.y), control2: CGPoint(x: mid, y: b.y))
        }
        let area = path.mutableCopy()!
        area.addLine(to: CGPoint(x: points.last!.x, y: 0))
        area.addLine(to: CGPoint(x: points[0].x, y: 0))
        area.closeSubpath()

        CATransaction.begin()
        CATransaction.setDisableActions(true)
        strip.frame = bounds
        fill.frame = bounds
        fillMask.frame = bounds
        line.frame = bounds
        line.path = path
        fillMask.path = area
        CATransaction.commit()

        if advanced {
            // Start where the previous sample was drawn, then glide over the sample interval.
            let glide = CABasicAnimation(keyPath: "transform.translation.x")
            glide.fromValue = step
            glide.toValue = 0
            glide.duration = 1
            strip.add(glide, forKey: "glide")
        }
    }
}

// MARK: - Formatting

enum Format {
    static func percent(_ value: Double) -> String {
        value < 10 ? String(format: "%.1f%%", value) : String(format: "%.0f%%", value)
    }

    /// Compact binary units: `0 B`, `812 KB`, `11.1 MB`.
    static func bytes(_ value: Double, locale: Locale = .current) -> String {
        let units = ["B", "KB", "MB", "GB", "TB"]
        var v = max(value, 0), i = 0
        while v >= 1024 && i < units.count - 1 { v /= 1024; i += 1 }
        let digits = i == 0 || v >= 100 ? 0 : 1
        return "\(v.formatted(.number.precision(.fractionLength(digits)).locale(locale))) \(units[i])"
    }

    /// Locale-neutral sizes for text an agent will read.
    static func plainBytes(_ value: Double) -> String { bytes(value, locale: Locale(identifier: "en_US_POSIX")) }

    static func rate(_ value: Double) -> String {
        "\(bytes(value))/s"
    }
}

// MARK: - Small pieces

// MARK: - Panes

/// The one surface every part of the detail view sits on: a soft glass card
/// with a small header (icon, title, controls) and content directly below.
struct Pane<Title: View, Accessory: View, Content: View>: View {
    let icon: String
    @ViewBuilder var title: Title
    @ViewBuilder var accessory: Accessory
    @ViewBuilder var content: Content

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: icon)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .frame(width: 16)
                    .contentTransition(.symbolEffect(.replace))
                title
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(PaneTitleStyle())
                    .lineLimit(1)
                Spacer(minLength: 8)
                accessory
            }
            .padding(.leading, 14)
            .padding(.trailing, 8)
            .frame(height: 38)
            content
        }
        .clipShape(.rect(cornerRadius: Pane.radius))
        .glassCard(cornerRadius: Pane.radius)
    }
}

extension Pane {
    nonisolated static var radius: CGFloat { 14 }
}

/// Primary in dark mode; a softer #333 on white cards in light mode.
private struct PaneTitleStyle: ShapeStyle {
    func resolve(in environment: EnvironmentValues) -> Color {
        environment.colorScheme == .light ? Color(white: 0.2) : .primary
    }
}

extension Pane where Title == Text {
    init(icon: String, title: String, @ViewBuilder accessory: () -> Accessory, @ViewBuilder content: () -> Content) {
        self.init(icon: icon, title: { Text(title) }, accessory: accessory, content: content)
    }
}

extension View {
    /// The pane surface. Dark: frosted glass with a hairline edge. Light:
    /// near-white cards with a soft, diffuse lift.
    func glassCard(cornerRadius: CGFloat) -> some View {
        modifier(PaneSurface(cornerRadius: cornerRadius))
    }
}

private struct PaneSurface: ViewModifier {
    let cornerRadius: CGFloat
    @Environment(\.colorScheme) private var scheme

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: cornerRadius)
        if scheme == .light {
            // Near-white cards lifted off the tinted glass by shadow, not by borders.
            content
                .background(Color.white.opacity(0.92), in: shape)
                .background(.regularMaterial, in: shape)
                .overlay(shape.strokeBorder(.black.opacity(0.035), lineWidth: 0.5))
                // Light enough that two neighbors' shadows don't darken the gap between them.
                .shadow(color: .black.opacity(0.05), radius: 10, y: 3)
                .shadow(color: .black.opacity(0.04), radius: 1.5, y: 0.5)
        } else {
            // No shadow: Liquid Glass-style shadows darken the gaps between panes.
            content
                .background(.thinMaterial, in: shape)
                .overlay(shape.strokeBorder(.white.opacity(0.08), lineWidth: 0.5))
        }
    }
}

extension View {
    /// Light mode only: a tinted layer over the window's blur. Its strength follows
    /// the wallpaper: thin over a light one (real glass), nearly opaque over a dark
    /// one (otherwise the blur turns light mode gray). Thicker when inactive.
    func lightGlass() -> some View {
        modifier(LightGlass())
    }
}

private struct LightGlass: ViewModifier {
    @Environment(\.colorScheme) private var scheme
    @Environment(\.controlActiveState) private var activeState

    func body(content: Content) -> some View {
        content.background {
            if scheme == .light {
                let base = 0.92 - 0.37 * Wallpaper.shared.luminance
                Color(red: 0.935, green: 0.925, blue: 0.945)
                    .opacity(min(base + (activeState == .inactive ? 0.05 : 0), 0.97))
                    .ignoresSafeArea()
            }
        }
    }
}

/// A glyph beside the window title, like a folder icon in Finder.
struct TitleIcon: View {
    let systemName: String

    var body: some View {
        Image(systemName: systemName)
            .font(.system(size: 18))
            .foregroundStyle(.secondary)
            .frame(width: 28, height: 28)
            .accessibilityHidden(true)
    }
}

/// Hover highlight for borderless icon buttons.
struct HoverButtonStyle: ButtonStyle {
    var tint: Color = .primary
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(tint)
            .padding(6)
            .background(.primary.opacity(configuration.isPressed ? 0.14 : hovering ? 0.08 : 0), in: .rect(cornerRadius: 6))
            .scaleEffect(configuration.isPressed ? 0.92 : 1)
            .animation(.snappy(duration: 0.18), value: configuration.isPressed)
            .animation(.easeOut(duration: 0.15), value: hovering)
            .onHover { hovering = $0 }
            .contentShape(.rect)
    }
}
