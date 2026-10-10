import SwiftUI

// MARK: - Sandbox

/// One sandbox: live stats, what the agent did, and what it changed.
struct SandboxDetail: View {
    @Environment(DockerStore.self) private var store
    let container: Container
    @State private var tab = Tab.activity
    @State private var journal = JournalWatcher()
    @State private var confirmDestroy = false

    enum Tab { case activity, changes }

    private var archivedAt: Date? { Journal.archivedAt(container.id) }
    private var commands: Int { journal.entries.filter { $0.kind == .exec }.count }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            StatsRow(stats: [store.stats(for: container.id)], running: container.state == .running)
            VStack(spacing: 0) {
                HStack {
                    Picker("View", selection: $tab) {
                        Text("Activity").tag(Tab.activity)
                        Text("Changes").tag(Tab.changes)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .fixedSize()
                    Spacer()
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                Divider().opacity(0.6)
                switch tab {
                case .activity: ActivityList(entries: journal.entries)
                case .changes: ChangesList(containerID: container.id, refresh: journal.entries.count)
                }
            }
            .panel()
        }
        .padding([.horizontal, .bottom], 20)
        .padding(.top, 8)
        .navigationTitle(container.name)
        .navigationSubtitle(subtitle)
        .toolbar {
            ToolbarItem(placement: .navigation) { TitleIcon(systemName: "flask.fill") }
            ToolbarItemGroup {
                if let docker = SandboxTerminal.dockerCLI {
                    Button { SandboxTerminal.open(container, docker: docker) } label: {
                        Label("Open in Terminal", systemImage: "terminal")
                    }
                    .help("Open a shell in this sandbox")
                    .disabled(!container.isRunning)
                }
                if archivedAt != nil {
                    Button { store.restore(sandbox: container) } label: { Label("Restore", systemImage: "arrow.uturn.backward") }
                        .help("Restore and start")
                } else {
                    Button { store.archive(sandbox: container) } label: { Label("Archive", systemImage: "archivebox") }
                        .help("Stop and keep for later")
                }
                Button { confirmDestroy = true } label: { Label("Destroy", systemImage: "trash") }
                    .help("Destroy")
            }
        }
        .task(id: container.id) { await journal.watch(container.id) }
        .confirmationDialog("Destroy \(container.name)?", isPresented: $confirmDestroy) {
            Button("Destroy", role: .destructive) { store.perform(.remove, on: container) }
        } message: {
            Text("The sandbox and everything in it will be deleted.")
        }
    }

    private var subtitle: String {
        if let archivedAt { return SandboxRetention.summary(archivedAt: archivedAt) }
        var parts = [container.image]
        if let client = container.sandboxClient { parts.append("by \(client)") }
        parts.append(commands == 1 ? "1 command" : "\(commands) commands")
        return parts.joined(separator: "  ·  ")
    }
}

/// Re-reads a sandbox's journal whenever the MCP server writes to it.
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

private struct ActivityList: View {
    let entries: [Journal.Entry]

    var body: some View {
        if entries.isEmpty {
            Text("Nothing yet. Commands your agent runs show up here.")
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
private struct ChangesList: View {
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

// MARK: - Overview

/// Shown when the Sandboxes group is selected: the command-line tool and retention.
struct SandboxesOverview: View {
    @Environment(DockerStore.self) private var store
    @AppStorage(SandboxRetention.key) private var retentionDays = 7
    @State private var installed = CLIInstaller.installedPath
    @State private var installFailed = false

    private var count: Int { store.containers.filter(\.isSandbox).count }

    private static let agentNote = "Use the `terrarium` CLI for throwaway Linux sandboxes (benchmarks, experiments, installing tools) instead of running them on this Mac. Run `terrarium --help` to see how."

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Command line")
                        .font(.system(size: 15, weight: .semibold))
                    Text("terrarium gives you and your agents disposable Linux sandboxes from any terminal: create one, run commands, copy files in and out, open a shell.")
                        .font(.system(size: 13))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)

                    if let installed {
                        Label("Installed at \(installed.abbreviatingHome)", systemImage: "checkmark.circle.fill")
                            .font(.system(size: 12.5))
                            .foregroundStyle(.secondary)
                            .symbolRenderingMode(.multicolor)
                            .transition(.opacity)
                    } else if installFailed {
                        Text("No folder on your PATH is writable. Run this in Terminal instead:")
                            .font(.system(size: 12.5))
                            .foregroundStyle(.secondary)
                        CopyableLine(text: CLIInstaller.manualCommand)
                    } else {
                        Button("Install Command-Line Tool") {
                            withAnimation(.snappy) {
                                if let path = try? CLIInstaller.install() { installed = path } else { installFailed = true }
                            }
                        }
                        .controlSize(.large)
                    }

                    Divider().padding(.vertical, 4)

                    Text("Tell your agent about it. Add this to CLAUDE.md or AGENTS.md:")
                        .font(.system(size: 12.5))
                        .foregroundStyle(.secondary)
                    CopyableLine(text: Self.agentNote, monospaced: false)
                }
                .padding(18)
                .panel()

                HStack {
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Archived sandboxes")
                            .font(.system(size: 13, weight: .medium))
                        Text("Sandboxes are kept until archived or removed. Archived ones are deleted after:")
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 16)
                    Picker("Delete after", selection: $retentionDays) {
                        ForEach(SandboxRetention.choices, id: \.self) { days in
                            Text(days == 0 ? "Never" : days == 1 ? "1 day" : "\(days) days").tag(days)
                        }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
                .padding(18)
                .panel()
            }
            .frame(maxWidth: 640, alignment: .leading)
            .padding(.horizontal, 20)
            .padding(.top, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .navigationTitle("Sandboxes")
        .navigationSubtitle(count == 0 ? "None yet" : count == 1 ? "1 sandbox" : "\(count) sandboxes")
        .toolbar { ToolbarItem(placement: .navigation) { TitleIcon(systemName: "flask.fill") } }
    }
}

/// A line of text with a Copy button that confirms itself.
private struct CopyableLine: View {
    let text: String
    var monospaced = true
    @State private var copied = false

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Text(text)
                .font(monospaced ? .system(size: 12).monospaced() : .system(size: 12.5))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(text, forType: .string)
                withAnimation(.snappy) { copied = true }
            } label: {
                Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                    .contentTransition(.symbolEffect(.replace))
            }
        }
        .padding(12)
        .background(.primary.opacity(0.05), in: .rect(cornerRadius: 8))
    }
}

// MARK: - Terminal

enum SandboxTerminal {
    static let dockerCLI = [
        "/opt/homebrew/bin/docker", "/usr/local/bin/docker", "/Applications/Docker.app/Contents/Resources/bin/docker",
    ].first { FileManager.default.isExecutableFile(atPath: $0) }

    /// Opens Terminal with a shell inside the sandbox, via a throwaway .command file.
    static func open(_ sandbox: Container, docker: String) {
        let script = """
        #!/bin/sh
        export DOCKER_HOST=unix://\(EnginePaths.socket.path)
        clear
        exec "\(docker)" exec -it -w \(Sandboxes.workspace) \(sandbox.id) sh -c 'command -v bash >/dev/null && exec bash -l || exec sh'
        """
        let url = FileManager.default.temporaryDirectory.appending(path: "terrarium-\(sandbox.name).command")
        try? script.write(to: url, atomically: true, encoding: .utf8)
        try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
        NSWorkspace.shared.open(url)
    }
}
