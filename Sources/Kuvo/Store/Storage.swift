import Foundation
import SwiftUI

/// What Kuvo takes on the Mac, what Docker's things take inside the VM, and getting
/// space back. Measured only when asked: sizing volumes walks every file in them.
@MainActor @Observable
final class Storage {
    /// The VM disk's allocated bytes: the one number that's true on the Mac.
    private(set) var onMac: Int64?
    private(set) var usage: DiskUsage?
    private(set) var measuring = false
    private(set) var updated: Date?
    private(set) var reclaiming = false
    /// What the last reclaim gave back on the Mac.
    private(set) var result: String?

    enum Category: CaseIterable, Hashable {
        case images, buildCache, containers, volumes
    }

    static func allocatedOnMac() -> Int64? {
        (try? EnginePaths.disk.resourceValues(forKeys: [.totalFileAllocatedSizeKey]).totalFileAllocatedSize).map(Int64.init)
    }

    func refresh(client: DockerClient?) async {
        guard !measuring else { return }
        measuring = true
        let usage = try? await client?.diskUsage()
        withAnimation(.snappy) {
            onMac = Self.allocatedOnMac()
            self.usage = usage
            updated = .now
            measuring = false
        }
    }

    /// Kuvo, Alpine and Docker themselves, the file system, and anything not yet trimmed.
    var system: Int64? {
        guard let onMac, let usage else { return nil }
        return max(onMac - usage.total, 0)
    }

    /// Stopped containers a reclaim may remove. Archived ones were kept on purpose
    /// and go only when their retention period ends.
    func stoppedContainers(_ containers: [Container]) -> [Container] {
        containers.filter { !$0.isRunning && $0.state != .paused && Archive.archivedAt($0.id) == nil }
    }

    /// Bytes a reclaim of each category frees inside the VM, at most: images can share layers.
    func reclaimable(_ category: Category, store: DockerStore) -> Int64 {
        guard let usage else { return 0 }
        switch category {
        // The same images the Images list calls unused, which is what a reclaim removes.
        case .images: return store.unusedImages.reduce(0) { $0 + $1.size }
        case .buildCache: return usage.reclaimableBuildCache
        case .volumes: return usage.unusedVolumes
        case .containers:
            let ids = Set(stoppedContainers(store.containers).map(\.id))
            return (usage.Containers ?? []).filter { ids.contains($0.Id) }.reduce(0) { $0 + max($1.SizeRw ?? 0, 0) }
        }
    }

    /// Removes the chosen things, trims, and reports what came back on the Mac. Runs in
    /// this order so removing containers can't make more images or volumes unused than
    /// the person saw when they chose.
    func reclaim(_ categories: Set<Category>, store: DockerStore, engine: Engine) async {
        guard let client = store.client, !reclaiming else { return }
        withAnimation(.snappy) {
            reclaiming = true
            result = nil
        }
        let before = Self.allocatedOnMac()
        var failed: [String] = []
        if categories.contains(.buildCache) {
            do { try await client.pruneBuildCache() } catch { failed.append("the build cache") }
        }
        if categories.contains(.images) {
            failed += await store.deleteImages(store.unusedImages)
        }
        if categories.contains(.containers) {
            for container in stoppedContainers(store.containers) {
                // Without force: one that started in the meantime stays.
                do {
                    _ = try await client.send("DELETE", "/containers/\(container.id)")
                    Archive.remove(container.id)
                } catch {
                    failed.append(container.name)
                }
            }
        }
        if categories.contains(.volumes) {
            do { try await client.pruneVolumes() } catch { failed.append("unused volumes") }
        }
        await engine.trim()
        let after = Self.allocatedOnMac()
        let freed = (before ?? 0) - (after ?? 0)
        withAnimation(.snappy) {
            result = freed >= 1 << 20 ? "Freed \(Format.bytes(Double(freed))) on your Mac" : "No space to free on your Mac"
            reclaiming = false
        }
        if !failed.isEmpty { store.failure = "Couldn't remove \(failed.formatted(.list(type: .and)))." }
        await refresh(client: client)
    }
}
