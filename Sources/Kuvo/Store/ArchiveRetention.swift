import Foundation

/// Archived containers are kept for a while, then deleted. Nothing else expires.
enum ArchiveRetention {
    static let key = "archive.retentionDays"
    static let choices = [1, 7, 30, 0] // 0 keeps them forever.

    /// The app's own defaults, so the CLI (a different process) sees the same setting.
    static var defaults: UserDefaults {
        Bundle.main.bundleIdentifier == suite ? .standard : UserDefaults(suiteName: suite) ?? .standard
    }
    private static let suite = "dev.kuvo.app"

    static var days: Int { defaults.object(forKey: key) as? Int ?? 7 }

    static func deadline(archivedAt: Date) -> Date? {
        days > 0 ? archivedAt.addingTimeInterval(Double(days) * 86_400) : nil
    }

    static func summary(archivedAt: Date) -> String {
        guard let deadline = deadline(archivedAt: archivedAt) else { return "Archived" }
        return "Archived, deleted \(deadline.formatted(.relative(presentation: .numeric)))"
    }

    /// Deletes expired archived containers, and records of containers that are gone.
    static func prune(_ containers: [Container], client: DockerClient) async {
        let service = ContainerService(client: client)
        for container in containers {
            if let archived = Archive.archivedAt(container.id), let deadline = deadline(archivedAt: archived), deadline < .now {
                try? await service.remove(container)
            }
        }
        let ids = Set(containers.map(\.id))
        let files = (try? FileManager.default.contentsOfDirectory(atPath: Archive.directory.path)) ?? []
        for file in files {
            let id = (file as NSString).deletingPathExtension
            if !ids.contains(id) { Archive.remove(id) }
        }
    }
}
