import SwiftUI

/// Settings › Storage: what Kuvo takes on the Mac, what's using it, and getting
/// space back. Opened from Kuvo › Storage… too.
struct StorageView: View {
    @Environment(DockerStore.self) private var store
    @Environment(Engine.self) private var engine
    @Environment(Storage.self) private var storage
    @Environment(Navigator.self) private var navigator
    @Environment(\.openWindow) private var openWindow
    @State private var reclaiming = false

    private var connected: Bool { if case .connected = store.connection { true } else { false } }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            headline
            UsageBar(segments: segments)
            VStack(spacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                    if index > 0 { Divider().padding(.leading, 30) }
                    row
                }
            }
            .padding(.vertical, 4)
            .padding(.horizontal, 12)
            .background(.background.secondary, in: .rect(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator, lineWidth: 0.5))
            footer
        }
        .padding(20)
        .frame(width: 540)
        .task { await storage.refresh(client: store.client) }
        // Re-measure after each trim, which follows anything Docker deletes, so both numbers move together.
        .onChange(of: engine.trims) { Task { await storage.refresh(client: store.client) } }
        .sheet(isPresented: $reclaiming) { ReclaimSheet() }
    }

    // MARK: Headline

    private var headline: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Kuvo Box uses")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                Text(storage.onMac.map { "\(Format.bytes(Double($0))) on your Mac" } ?? "—")
                    .font(.system(size: 22, weight: .semibold, design: .rounded).monospacedDigit())
                    .contentTransition(.numericText())
            }
            Spacer()
            HStack(spacing: 4) {
                Text(status)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .contentTransition(.interpolate)
                Button {
                    Task { await storage.refresh(client: store.client) }
                } label: {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 11, weight: .medium))
                        .symbolEffect(.rotate, value: storage.measuring)
                }
                .buttonStyle(HoverButtonStyle())
                .help("Measure again")
                .disabled(storage.measuring || !connected)
            }
        }
    }

    private var status: String {
        if storage.measuring { return "Measuring…" }
        guard let updated = storage.updated else { return "" }
        return Date.now.timeIntervalSince(updated) < 60 ? "Updated just now" : "Updated \(updated.formatted(.relative(presentation: .named)))"
    }

    // MARK: Breakdown

    private struct Row: View {
        let color: Color
        let title: String
        let size: Int64?
        let detail: String?
        var show: (() -> Void)?

        var body: some View {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                RoundedRectangle(cornerRadius: 2).fill(color).frame(width: 8, height: 8)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.system(size: 13))
                    if let detail {
                        Text(detail).font(.system(size: 11)).foregroundStyle(.secondary)
                    }
                }
                Spacer(minLength: 8)
                Text(size.map { Format.bytes(Double($0)) } ?? "—")
                    .font(.system(size: 13).monospacedDigit())
                    .foregroundStyle(.secondary)
                    .contentTransition(.numericText())
                if let show {
                    Button("Show", action: show)
                        .buttonStyle(.link)
                        .font(.system(size: 12))
                        .help("Show \(title.lowercased()) in the main window")
                } else {
                    // Keeps the sizes aligned.
                    Text("Show").font(.system(size: 12)).hidden().accessibilityHidden(true)
                }
            }
            .padding(.vertical, 8)
        }
    }

    private static let colors: [Color] = [.accentColor, .accentColor.opacity(0.7), .accentColor.opacity(0.45), .accentColor.opacity(0.25), .gray.opacity(0.7)]

    private var rows: [Row] {
        let usage = storage.usage
        func bytes(_ value: Int64) -> String { Format.bytes(Double(value)) }
        func part(_ value: Int64, _ text: String) -> String? { value > 0 ? "\(bytes(value)) \(text)" : nil }
        return [
            Row(color: Self.colors[0], title: "Images", size: usage?.images,
                detail: usage == nil ? nil : part(storage.reclaimable(.images, store: store), "not used by any container"), show: { show(.images) }),
            Row(color: Self.colors[1], title: "Containers", size: usage?.containers,
                detail: usage == nil ? nil : part(storage.reclaimable(.containers, store: store), "in stopped containers"),
                show: { show(.containers) }),
            Row(color: Self.colors[2], title: "Volumes", size: usage?.volumes,
                detail: usage.flatMap { part($0.unusedVolumes, "not in use") }, show: { show(.volumes) }),
            Row(color: Self.colors[3], title: "Build cache", size: usage?.buildCache,
                detail: usage?.buildCacheLastUsed.map { "Last used \($0.formatted(.relative(presentation: .named)))" }),
            Row(color: Self.colors[4], title: "System", size: storage.system,
                detail: connected ? "Linux, Docker and the VM's file system" : "Start the engine to see what's using space"),
        ]
    }

    private var segments: [UsageBar.Segment] {
        guard let usage = storage.usage, let onMac = storage.onMac, onMac > 0 else { return [] }
        let sizes = [usage.images, usage.containers, usage.volumes, usage.buildCache, storage.system ?? 0]
        return zip(sizes, Self.colors).map { UsageBar.Segment(fraction: Double($0) / Double(max(onMac, usage.total)), color: $1) }
    }

    private func show(_ section: SidebarSection) {
        navigator.section = section
        openWindow(id: "main")
        NSApp.activate()
    }

    // MARK: Footer

    private var footer: some View {
        HStack {
            if storage.reclaiming {
                ProgressView().controlSize(.small)
                Text("Reclaiming space…").font(.system(size: 12)).foregroundStyle(.secondary)
            } else if let result = storage.result {
                Label(result, systemImage: "checkmark.circle.fill")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .transition(.opacity)
            }
            Spacer()
            Button("Reclaim Space…") { reclaiming = true }
                .disabled(!canReclaim)
                .help(canReclaim ? "Choose what to remove" : "Nothing to remove right now")
        }
    }

    private var canReclaim: Bool {
        connected && !storage.reclaiming
            && Storage.Category.allCases.contains { storage.reclaimable($0, store: store) > 0 }
    }
}

/// One bar split by what uses the space.
private struct UsageBar: View {
    struct Segment {
        let fraction: Double
        let color: Color
    }

    let segments: [Segment]

    var body: some View {
        GeometryReader { geometry in
            HStack(spacing: 1) {
                ForEach(Array(segments.enumerated()), id: \.offset) { _, segment in
                    if segment.fraction > 0 {
                        Rectangle()
                            .fill(segment.color)
                            .frame(width: max(geometry.size.width * segment.fraction - 1, 2))
                    }
                }
                Spacer(minLength: 0)
            }
        }
        .frame(height: 10)
        .background(.primary.opacity(0.06))
        .clipShape(.capsule)
        .animation(.snappy, value: segments.map(\.fraction))
        .accessibilityHidden(true)
    }
}

/// Picks what to remove. Things that come back on their own (images download
/// again, build cache rebuilds) are on by default; containers' changes and
/// volumes' data don't come back, so those are left for the person to choose.
private struct ReclaimSheet: View {
    @Environment(DockerStore.self) private var store
    @Environment(Engine.self) private var engine
    @Environment(Storage.self) private var storage
    @Environment(\.dismiss) private var dismiss
    @State private var chosen: Set<Storage.Category> = [.images, .buildCache]

    private struct Choice {
        let category: Storage.Category
        let title: String
        let note: String
    }

    private let choices = [
        Choice(category: .images, title: "Unused images", note: "Downloaded again when a container needs them."),
        Choice(category: .buildCache, title: "Build cache", note: "Rebuilt on the next build."),
        Choice(category: .containers, title: "Stopped containers", note: "Their changes and logs are lost. Archived containers are kept."),
        Choice(category: .volumes, title: "Unused volumes", note: "Deletes the data in them. This can't be undone."),
    ]

    private func size(_ category: Storage.Category) -> Int64 { storage.reclaimable(category, store: store) }
    private var total: Int64 { chosen.reduce(0) { $0 + size($1) } }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Reclaim Space").font(.headline)
            VStack(alignment: .leading, spacing: 12) {
                ForEach(choices, id: \.category) { choice in
                    Toggle(isOn: Binding(
                        // Nothing to free shows unchecked, whatever the default.
                        get: { chosen.contains(choice.category) && size(choice.category) > 0 },
                        set: { if $0 { chosen.insert(choice.category) } else { chosen.remove(choice.category) } }
                    )) {
                        VStack(alignment: .leading, spacing: 2) {
                            HStack {
                                Text(choice.title)
                                Spacer()
                                Text(Format.bytes(Double(size(choice.category))))
                                    .monospacedDigit()
                                    .foregroundStyle(.secondary)
                            }
                            Text(choice.note)
                                .font(.system(size: 11))
                                .foregroundStyle(.secondary)
                        }
                    }
                    .toggleStyle(.checkbox)
                    .disabled(size(choice.category) == 0)
                }
            }
            Text("Images can share layers, so less space may be freed than these sizes add up to.")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Reclaim \(Format.bytes(Double(total)))") {
                    let categories = chosen.filter { size($0) > 0 }
                    dismiss()
                    Task { await storage.reclaim(categories, store: store, engine: engine) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(total == 0)
            }
        }
        .padding(20)
        .frame(width: 420)
    }
}
