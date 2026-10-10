import AppKit
import SwiftUI

/// A container whose logs to show; `label` prefixes each line when merging several.
struct LogSource: Hashable, Sendable {
    let id: String
    var label: String?
}

struct LogsView: View {
    @Environment(DockerStore.self) private var store
    let sources: [LogSource]
    var isRunning = true
    @State private var model = LogModel()
    @AppStorage("logs.timestamps") private var timestamps = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 2) {
                Text("Logs")
                    .font(.system(size: 13, weight: .semibold))
                Text("\(model.lineCount) lines")
                    .font(.system(size: 11).monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .contentTransition(.numericText(value: Double(model.lineCount)))
                    .animation(.snappy, value: model.lineCount)
                    .padding(.leading, 6)
                Spacer()
                Button { timestamps.toggle() } label: {
                    Image(systemName: "clock")
                }
                .buttonStyle(HoverButtonStyle(tint: timestamps ? .accentColor : .secondary))
                .help(timestamps ? "Hide timestamps" : "Show timestamps")
                Button { model.clear() } label: { Image(systemName: "trash") }
                    .buttonStyle(HoverButtonStyle(tint: .secondary))
                    .help("Clear")
            }
            .font(.system(size: 12))
            .padding(.leading, 14)
            .padding(.trailing, 8)
            .padding(.vertical, 6)

            Divider().opacity(0.6)

            LogTextView(model: model)
                .overlay(alignment: .bottomTrailing) {
                    if !model.isFollowing {
                        Button { model.scrollToEnd() } label: {
                            Label("Latest", systemImage: "arrow.down")
                                .font(.system(size: 11, weight: .medium))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 5)
                                .background(.regularMaterial, in: .capsule)
                                .overlay(Capsule().strokeBorder(.separator, lineWidth: 0.5))
                                .shadow(color: .black.opacity(0.12), radius: 6, y: 2)
                        }
                        .buttonStyle(.plain)
                        .padding(12)
                        .transition(.scale(0.8).combined(with: .opacity))
                    }
                }
                .animation(.snappy, value: model.isFollowing)
                .overlay {
                    if model.lineCount == 0 && model.loaded {
                        Text(isRunning ? "Waiting for output…" : "No output")
                            .font(.system(size: 12))
                            .foregroundStyle(.tertiary)
                            .transition(.opacity)
                    }
                }
        }
        .panel()
        .onChange(of: timestamps, initial: true) { model.showTimestamps = timestamps }
        .task(id: sources) {
            guard let client = store.client else { return }
            await model.follow(client: client, sources: sources) { [store] id in
                store.containers.first { $0.id == id }?.isRunning ?? false
            }
        }
    }
}

// MARK: - Model

@MainActor @Observable
final class LogModel {
    private(set) var lineCount = 0
    private(set) var isFollowing = true
    private(set) var loaded = false
    var showTimestamps = false { didSet { if oldValue != showTimestamps { rebuild() } } }

    @ObservationIgnored weak var textView: NSTextView?
    @ObservationIgnored private var lines: [LogLine] = []
    @ObservationIgnored private var lengths: [Int] = []
    @ObservationIgnored private var labels: [String] = []
    @ObservationIgnored private var labelWidth = 0
    private static let cap = 5000

    func follow(client: DockerClient, sources: [LogSource], isRunning: @escaping @MainActor (String) -> Bool) async {
        lines = []; lengths = []; lineCount = 0; loaded = false
        labels = sources.map { $0.label ?? "" }
        labelWidth = labels.map(\.count).max() ?? 0
        textView?.textStorage?.setAttributedString(NSAttributedString())

        // Backlog first, merged by time so a project's services interleave.
        let start = Date()
        var backlog: [LogLine] = []
        var since: [String: Date] = [:]
        await withTaskGroup(of: (String, [LogLine]).self) { group in
            for (index, source) in sources.enumerated() {
                group.addTask { (source.id, await Self.read(client, source.id, index: index, query: "tail=1000")) }
            }
            for await (id, lines) in group {
                backlog += lines
                since[id] = lines.last?.date ?? start
            }
        }
        if sources.count > 1 {
            backlog = backlog.enumerated()
                .sorted { ($0.element.date ?? .distantPast, $0.offset) < ($1.element.date ?? .distantPast, $1.offset) }
                .map(\.element)
        }
        append(backlog)
        loaded = true

        // Then follow each source live.
        await withTaskGroup(of: Void.self) { group in
            for (index, source) in sources.enumerated() {
                let start = since[source.id] ?? start
                group.addTask { await self.tail(client, source.id, index: index, since: start, isRunning: isRunning) }
            }
        }
    }

    /// Follows one container, resuming after restarts.
    private func tail(_ client: DockerClient, _ id: String, index: Int, since: Date, isRunning: @MainActor (String) -> Bool) async {
        var since = since
        repeat {
            var decoder = LogDecoder()
            let stream = client.stream("GET", Self.path(id, "follow=true&since=\(since.timeIntervalSince1970)"))
            do {
                for try await chunk in stream {
                    var batch = decoder.feed(chunk).map { var line = $0; line.source = index; return line }
                    batch.removeAll { ($0.date ?? .distantFuture) <= since }
                    append(batch)
                    since = batch.last?.date ?? since
                }
            } catch {}
            try? await Task.sleep(for: .seconds(1))
        } while !Task.isCancelled && isRunning(id)
    }

    private nonisolated static func read(_ client: DockerClient, _ id: String, index: Int, query: String) async -> [LogLine] {
        var decoder = LogDecoder()
        var lines: [LogLine] = []
        do {
            for try await chunk in client.stream("GET", path(id, query)) {
                lines += decoder.feed(chunk).map { var line = $0; line.source = index; return line }
            }
        } catch {}
        return lines
    }

    private nonisolated static func path(_ id: String, _ query: String) -> String {
        "/containers/\(id)/logs?stdout=true&stderr=true&timestamps=true&\(query)"
    }

    func clear() {
        lines = []; lengths = []
        textView?.textStorage?.setAttributedString(NSAttributedString())
        withAnimation(.snappy) { lineCount = 0 }
    }

    func scrollToEnd() {
        textView?.scrollToEndOfDocument(nil)
        isFollowing = true
    }

    /// Only user scrolling can stop following; reaching the bottom by any means resumes it.
    func scrolled(byUser: Bool) {
        guard let textView, let visible = textView.enclosingScrollView?.documentVisibleRect else { return }
        let atBottom = visible.maxY >= textView.bounds.maxY - 24
        if atBottom && !isFollowing { isFollowing = true }
        if byUser && !atBottom && isFollowing { isFollowing = false }
    }

    func rebuild() {
        guard let storage = textView?.textStorage else { return }
        let rendered = lines.map(render)
        lengths = rendered.map(\.length)
        let all = NSMutableAttributedString()
        rendered.forEach(all.append)
        storage.setAttributedString(all)
        if isFollowing { textView?.scrollToEndOfDocument(nil) }
    }

    private func append(_ batch: [LogLine]) {
        guard !batch.isEmpty else { return }
        lines += batch
        lineCount = lines.count
        guard let storage = textView?.textStorage else { return }

        let chunk = NSMutableAttributedString()
        for line in batch {
            let text = render(line)
            lengths.append(text.length)
            chunk.append(text)
        }
        storage.beginEditing()
        storage.append(chunk)
        if lines.count > Self.cap {
            let drop = lines.count - Self.cap
            let length = lengths.prefix(drop).reduce(0, +)
            storage.deleteCharacters(in: NSRange(location: 0, length: length))
            lines.removeFirst(drop)
            lengths.removeFirst(drop)
            lineCount = lines.count
        }
        storage.endEditing()
        if isFollowing { textView?.scrollToEndOfDocument(nil) }
    }

    private static let timeFormat = Date.FormatStyle().hour(.twoDigits(amPM: .omitted)).minute().second()

    private func render(_ line: LogLine) -> NSAttributedString {
        let out = NSMutableAttributedString()
        if showTimestamps, let date = line.date {
            out.append(NSAttributedString(string: date.formatted(Self.timeFormat) + "  ", attributes: [
                .font: ANSI.font, .foregroundColor: NSColor.tertiaryLabelColor, .paragraphStyle: ANSI.paragraph,
            ]))
        }
        if labelWidth > 0, let source = line.source, labels.indices.contains(source) {
            let label = labels[source].padding(toLength: labelWidth, withPad: " ", startingAt: 0)
            out.append(NSAttributedString(string: label + "  ", attributes: [
                .font: ANSI.bold, .foregroundColor: ANSI.sourceColor(source), .paragraphStyle: ANSI.paragraph,
            ]))
        }
        out.append(ANSI.render(line.text))
        out.append(NSAttributedString(string: "\n", attributes: [.font: ANSI.font]))
        return out
    }
}

// MARK: - AppKit text view

private struct LogTextView: NSViewRepresentable {
    let model: LogModel

    func makeNSView(context: Context) -> NSScrollView {
        let scrollView = UserScrollView()
        // Borrow AppKit's preconfigured (TextKit 2) text view, host it in our scroll view.
        let textView = NSTextView.scrollableTextView().documentView as! NSTextView
        scrollView.documentView = textView
        scrollView.hasVerticalScroller = true
        scrollView.onUserScroll = { [weak model] in model?.scrolled(byUser: true) }
        scrollView.drawsBackground = false
        scrollView.scrollerStyle = .overlay
        scrollView.autohidesScrollers = true

        textView.isEditable = false
        textView.isSelectable = true
        textView.drawsBackground = false
        textView.isRichText = true
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.textContainerInset = NSSize(width: 10, height: 10)
        textView.font = ANSI.font

        scrollView.contentView.postsBoundsChangedNotifications = true
        context.coordinator.observer = NotificationCenter.default.addObserver(
            forName: NSView.boundsDidChangeNotification, object: scrollView.contentView, queue: .main
        ) { [weak model] _ in MainActor.assumeIsolated { model?.scrolled(byUser: false) } }

        model.textView = textView
        model.rebuild()
        return scrollView
    }

    func updateNSView(_ nsView: NSScrollView, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator {
        var observer: NSObjectProtocol?
        deinit { observer.map(NotificationCenter.default.removeObserver) }
    }
}

/// Distinguishes user scrolling (wheel, trackpad) from programmatic scrolling.
private final class UserScrollView: NSScrollView {
    var onUserScroll: (@MainActor () -> Void)?

    override func scrollWheel(with event: NSEvent) {
        super.scrollWheel(with: event)
        onUserScroll?()
    }
}

// MARK: - ANSI

/// Minimal SGR (color/bold/dim) support, plus level keyword highlighting.
@MainActor enum ANSI {
    static let font = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)
    static let bold = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .semibold)
    static let paragraph: NSParagraphStyle = {
        let style = NSMutableParagraphStyle()
        style.lineHeightMultiple = 1.18
        return style
    }()

    private static let palette: [Int: NSColor] = [
        30: .secondaryLabelColor, 31: .systemRed, 32: .systemGreen, 33: .systemOrange,
        34: .systemBlue, 35: .systemPurple, 36: .systemTeal, 37: .labelColor,
        90: .tertiaryLabelColor, 91: .systemRed, 92: .systemGreen, 93: .systemYellow,
        94: .systemBlue, 95: .systemPink, 96: .systemCyan, 97: .labelColor,
    ]

    /// Stable per-service colors for merged logs, like `docker compose logs`.
    static func sourceColor(_ index: Int) -> NSColor {
        let colors: [NSColor] = [.systemBlue, .systemPurple, .systemTeal, .systemOrange, .systemPink, .systemGreen, .systemIndigo, .systemBrown]
        return colors[index % colors.count]
    }

    private static let levels: [(String, NSColor)] = [
        ("ERROR", .systemRed), ("FATAL", .systemRed), ("PANIC", .systemRed),
        ("WARNING", .systemOrange), ("WARN", .systemOrange),
    ]

    static func render(_ text: String) -> NSAttributedString {
        let out = NSMutableAttributedString()
        var color = NSColor.labelColor
        var weight = font
        var segment = ""
        var hadEscape = false

        func flush() {
            guard !segment.isEmpty else { return }
            out.append(NSAttributedString(string: segment, attributes: [.font: weight, .foregroundColor: color, .paragraphStyle: paragraph]))
            segment = ""
        }

        var i = text.startIndex
        while i < text.endIndex {
            let ch = text[i]
            guard ch == "\u{1B}", text.index(after: i) < text.endIndex, text[text.index(after: i)] == "[" else {
                segment.append(ch)
                i = text.index(after: i)
                continue
            }
            hadEscape = true
            // Parse CSI: ESC [ params final-byte
            var j = text.index(i, offsetBy: 2)
            var params = ""
            while j < text.endIndex, let ascii = text[j].asciiValue, (0x30...0x3F).contains(ascii) {
                params.append(text[j]); j = text.index(after: j)
            }
            guard j < text.endIndex else { break }
            if text[j] == "m" {
                flush()
                for code in params.split(separator: ";").compactMap({ Int($0) }).ifEmpty([0]) {
                    switch code {
                    case 0: color = .labelColor; weight = font
                    case 1: weight = bold
                    case 2: color = .secondaryLabelColor
                    case 22: weight = font
                    case 39: color = .labelColor
                    default: if let c = palette[code] { color = c }
                    }
                }
            }
            i = text.index(after: j)
        }
        flush()

        if !hadEscape {
            for (word, tint) in levels {
                if let range = text.range(of: word) {
                    let ns = NSRange(range, in: text)
                    out.addAttributes([.foregroundColor: tint, .font: bold], range: ns)
                    break
                }
            }
        }
        return out
    }
}

private extension Array {
    func ifEmpty(_ fallback: [Element]) -> [Element] { isEmpty ? fallback : self }
}
