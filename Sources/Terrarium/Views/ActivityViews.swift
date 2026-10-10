import SwiftUI

// MARK: - Activity

/// Re-reads a container's activity whenever the CLI records something.
@MainActor @Observable
final class JournalWatcher {
    private(set) var entries: [Journal.Entry] = []

    func watch(_ id: String) async {
        entries = Journal.entries(id)
        var seen = Journal.modified(id)
        while !Task.isCancelled {
            try? await Task.sleep(for: .milliseconds(400))
            let modified = Journal.modified(id)
            guard modified != seen else { continue }
            seen = modified
            let fresh = Journal.entries(id)
            if fresh != entries { withAnimation(.snappy) { entries = fresh } }
        }
    }
}

// MARK: - Activity

struct ActivityList: View {
    let entries: [Journal.Entry]

    var body: some View {
        if entries.isEmpty {
            Text("No activity yet.")
                .font(.system(size: 12))
                .foregroundStyle(.tertiary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(entries) { entry in
                        ActivityRow(entry: entry)
                            .transition(.move(edge: .bottom).combined(with: .opacity))
                        Divider().opacity(0.4)
                    }
                }
            }
            // Fill from the top; once it overflows, stay pinned to the newest entry.
            .defaultScrollAnchor(.top, for: .alignment)
            .defaultScrollAnchor(.bottom, for: .initialOffset)
            .defaultScrollAnchor(.bottom, for: .sizeChanges)
        }
    }
}

private struct ActivityRow: View {
    let entry: Journal.Entry
    @State private var expanded = false

    private var hasOutput: Bool { !(entry.output ?? "").isEmpty }
    private var failed: Bool { (entry.exitCode ?? 0) != 0 }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                glyph.frame(width: 14)
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.title)
                        .font(entry.kind == .exec ? .system(size: 12).monospaced() : .system(size: 12.5))
                        .lineLimit(expanded ? nil : 2)
                        .textSelection(.enabled)
                    if let detail = entry.detail {
                        Text(detail).font(.system(size: 11)).foregroundStyle(.secondary)
                    }
                }
                Spacer(minLength: 12)
                meta
            }
            if expanded, let output = entry.output, !output.isEmpty {
                ScrollView {
                    Text(output)
                        .font(.system(size: 11.5).monospaced())
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10)
                }
                .frame(maxHeight: 280)
                .fixedSize(horizontal: false, vertical: true)
                .background(.primary.opacity(0.04), in: .rect(cornerRadius: 8))
                .padding(.leading, 24)
                .transition(.opacity)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .contentShape(.rect)
        .onTapGesture { if hasOutput { withAnimation(.snappy) { expanded.toggle() } } }
        .pointerStyle(hasOutput ? .link : nil)
    }

    @ViewBuilder private var glyph: some View {
        switch entry.kind {
        case .exec where !entry.finished:
            ProgressView().controlSize(.mini)
        case .exec where entry.exitCode == nil:
            Image(systemName: "clock").foregroundStyle(.orange)
        case .exec:
            Image(systemName: failed ? "xmark" : "checkmark")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(failed ? Color.red : Color.green)
        case .write:
            Image(systemName: "doc").foregroundStyle(.secondary)
        case .copy:
            Image(systemName: "square.and.arrow.down").foregroundStyle(.secondary)
        case .create:
            Image(systemName: "sparkle").foregroundStyle(.secondary)
        }
    }

    private var meta: some View {
        HStack(spacing: 10) {
            if failed, let code = entry.exitCode {
                Text("exit \(code)").foregroundStyle(.red)
            }
            if entry.kind == .exec {
                if entry.finished, let duration = entry.duration {
                    Text(Self.duration(duration))
                } else if !entry.finished {
                    TimelineView(.periodic(from: .now, by: 1)) { context in
                        Text(Self.duration(context.date.timeIntervalSince(entry.date)))
                    }
                }
            }
            Text(entry.date.formatted(date: .omitted, time: .standard))
                .foregroundStyle(.tertiary)
        }
        .font(.system(size: 11).monospacedDigit())
        .foregroundStyle(.secondary)
    }

    private static func duration(_ seconds: Double) -> String {
        seconds < 1 ? "\(Int(seconds * 1000)) ms"
            : seconds < 60 ? String(format: "%.1f s", seconds)
            : "\(Int(seconds) / 60)m \(Int(seconds) % 60)s"
    }
}

// MARK: - Changes

/// Files the agent added, modified or deleted, compared with the image.
struct ChangesList: View {
    @Environment(DockerStore.self) private var store
    let containerID: String
    let refresh: Int
    @State private var changes: [Change] = []
    @State private var showSystem = false

    struct Change: Decodable, Hashable {
        let Path: String
        let Kind: Int // 0 modified, 1 added, 2 deleted
    }

    private static let systemPrefixes = ["/usr/", "/var/", "/etc/", "/lib/", "/bin/", "/sbin/", "/run/", "/tmp/", "/root/."]

    /// Leaves only: a changed directory is implied by the files inside it.
    private var leaves: [Change] {
        var parents = Set<String>()
        for change in changes {
            var path = (change.Path as NSString).deletingLastPathComponent
            while path.count > 1 { parents.insert(path); path = (path as NSString).deletingLastPathComponent }
        }
        return changes.filter { !parents.contains($0.Path) }
    }

    private var visible: [Change] {
        showSystem ? leaves : leaves.filter { change in !Self.systemPrefixes.contains { change.Path.hasPrefix($0) } }
    }

    var body: some View {
        VStack(spacing: 0) {
            if visible.isEmpty {
                Text(leaves.isEmpty ? "No changes yet." : "No changes outside system folders.")
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(visible, id: \.self) { change in
                            HStack(spacing: 10) {
                                Text(["M", "A", "D"][min(change.Kind, 2)])
                                    .font(.system(size: 10.5, weight: .bold).monospaced())
                                    .foregroundStyle([Color.orange, .green, .red][min(change.Kind, 2)])
                                    .frame(width: 14)
                                Text(change.Path)
                                    .font(.system(size: 12).monospaced())
                                    .strikethrough(change.Kind == 2, color: .secondary)
                                    .textSelection(.enabled)
                                Spacer()
                            }
                            .padding(.horizontal, 14)
                            .padding(.vertical, 4)
                        }
                    }
                    .padding(.vertical, 6)
                }
            }
            Divider().opacity(0.6)
            HStack {
                Toggle("Show system folders", isOn: $showSystem)
                    .toggleStyle(.checkbox)
                Spacer()
                Text(visible.count == 1 ? "1 change" : "\(visible.count) changes")
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            .font(.system(size: 11.5))
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
        }
        .task(id: refresh) {
            guard let client = store.client,
                  let data = try? await client.send("GET", "/containers/\(containerID)/changes") else { return }
            let fresh = (try? JSONDecoder().decode([Change]?.self, from: data)) ?? nil
            withAnimation(.snappy) { changes = (fresh ?? []).sorted { $0.Path < $1.Path } }
        }
    }
}

// MARK: - Terminal

/// Opens Terminal with a shell inside a container, via our own CLI.
enum TerminalLauncher {
    static func open(_ container: Container) {
        let script = """
        #!/bin/sh
        clear
        exec '\(CLIInstaller.executable)' shell \(container.id)
        """
        let url = FileManager.default.temporaryDirectory.appending(path: "terrarium-\(container.name).command")
        try? script.write(to: url, atomically: true, encoding: .utf8)
        try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
        NSWorkspace.shared.open(url)
    }
}
