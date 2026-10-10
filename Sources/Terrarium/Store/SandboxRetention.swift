import Foundation

/// Archived sandboxes are kept for a while, then deleted. Nothing else expires.
enum SandboxRetention {
    static let key = "sandboxes.retentionDays"
    static let choices = [1, 7, 30, 0] // 0 keeps them forever.

    static var days: Int { UserDefaults.standard.object(forKey: key) as? Int ?? 7 }

    static func deadline(archivedAt: Date) -> Date? {
        days > 0 ? archivedAt.addingTimeInterval(Double(days) * 86_400) : nil
    }

    static func summary(archivedAt: Date) -> String {
        guard let deadline = deadline(archivedAt: archivedAt) else { return "Archived" }
        return "Archived, deleted \(deadline.formatted(.relative(presentation: .named)))"
    }

    /// Deletes expired archived sandboxes and journals whose container is gone.
    static func prune(_ containers: [Container], client: DockerClient) async {
        let sandboxes = Sandboxes(client: client)
        for container in containers where container.isSandbox {
            if let archived = Journal.archivedAt(container.id), let deadline = deadline(archivedAt: archived), deadline < .now {
                try? await sandboxes.destroy(container)
            }
        }
        let ids = Set(containers.map(\.id))
        let files = (try? FileManager.default.contentsOfDirectory(atPath: Journal.directory.path)) ?? []
        for file in files {
            let id = (file as NSString).deletingPathExtension
            if !ids.contains(id) { Journal.remove(id) }
        }
    }
}
