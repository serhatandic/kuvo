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
                Circle()
                    .fill(state.color.opacity(0.35))
                    .phaseAnimator([false, true]) { halo, expanded in
                        halo.scaleEffect(expanded ? 2.2 : 1).opacity(expanded ? 0 : 0.9)
                    } animation: { expanded in
                        expanded ? .easeOut(duration: 1.8) : .linear(duration: 0)
                    }
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

// MARK: - Sparkline

/// A sparkline that scrolls continuously: when a sample arrives the whole line
/// glides one step to the left over the sample interval, like Activity Monitor.
struct SparklineShape: Shape {
    var values: [Double]
    var window: Int
    var tick: Double
    var target: Double
    var ceiling: Double
    var closed = false

    var animatableData: AnimatablePair<Double, Double> {
        get { AnimatablePair(tick, ceiling) }
        set { tick = newValue.first; ceiling = newValue.second }
    }

    func path(in rect: CGRect) -> Path {
        var path = Path()
        guard values.count > 1, ceiling > 0 else {
            if closed { return path }
            path.move(to: CGPoint(x: rect.minX, y: rect.maxY - 0.75))
            path.addLine(to: CGPoint(x: rect.maxX, y: rect.maxY - 0.75))
            return path
        }
        let step = rect.width / CGFloat(window - 1)
        let shift = CGFloat(min(max(target - tick, 0), 1)) * step
        let inset: CGFloat = 1.5
        let points = values.enumerated().map { i, v in
            CGPoint(
                x: rect.maxX + shift - CGFloat(values.count - 1 - i) * step,
                y: rect.maxY - inset - CGFloat(min(v / ceiling, 1)) * (rect.height - inset * 2)
            )
        }
        path.move(to: points[0])
        for (a, b) in zip(points, points.dropFirst()) {
            let mid = (a.x + b.x) / 2
            path.addCurve(to: b, control1: CGPoint(x: mid, y: a.y), control2: CGPoint(x: mid, y: b.y))
        }
        if closed {
            path.addLine(to: CGPoint(x: points.last!.x, y: rect.maxY))
            path.addLine(to: CGPoint(x: points[0].x, y: rect.maxY))
            path.closeSubpath()
        }
        return path
    }
}

struct Sparkline: View {
    let values: [Double]
    let tick: Int
    var minimum: Double = 1
    var color: Color = .accentColor
    var lineWidth: CGFloat = 1.5
    /// Levels like memory read better padded with their first sample; rates with zero.
    var isLevel = false

    /// Rounded up so the scale doesn't twitch with every sample.
    private var ceiling: Double {
        let peak = max(values.max() ?? 0, minimum) * 1.15
        let magnitude = pow(10, (log10(peak)).rounded(.down))
        return (peak / magnitude * 2).rounded(.up) / 2 * magnitude
    }

    @Environment(\.colorScheme) private var scheme

    var body: some View {
        // Pad to a full window so a fresh series reads as a flat line with data flowing in.
        let padded = Array(repeating: isLevel ? values.first ?? 0 : 0, count: max(LiveStats.window + 1 - values.count, 0)) + values
        let line = SparklineShape(values: padded, window: LiveStats.window, tick: Double(tick), target: Double(tick), ceiling: ceiling)
        var area = line
        area.closed = true
        return ZStack {
            // Lighter on white cards, where color reads heavier than on dark glass.
            area.fill(LinearGradient(colors: [color.opacity(scheme == .light ? 0.18 : 0.28), color.opacity(0)], startPoint: .top, endPoint: .bottom))
            line.stroke(color, style: StrokeStyle(lineWidth: scheme == .light ? lineWidth * 0.85 : lineWidth, lineCap: .round, lineJoin: .round))
        }
        .clipped()
        .animation(.linear(duration: 1), value: tick)
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
    /// Light mode only: a faint tinted layer over the window's blur, so the glass
    /// stays bright over a dark desktop. Thicker when the window is inactive,
    /// since materials flatten then.
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
                Color(red: 0.925, green: 0.915, blue: 0.935)
                    .opacity(activeState == .inactive ? 0.8 : 0.66)
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
