import Foundation
import SwiftUI

@MainActor @Observable
final class DockerStore {
    enum Connection: Equatable {
        case connecting
        case connected(engine: String)
        case unavailable(String)
    }

    private(set) var connection = Connection.connecting
    private(set) var containers: [Container] = []
    private(set) var busy: [String: ContainerAction] = [:]
    /// Nil until loaded: volumes are only fetched while they're on screen (see `wantsVolumes`).
    private(set) var volumes: [Volume]?
    private(set) var removingVolumes: Set<String> = []
    /// Nil until loaded. Unlike volumes, listing images is cheap, so they're kept
    /// current whenever Docker is: that's how Kuvo sees a tag move to a newer image.
    private(set) var images: [DockerImage]?
    private(set) var removingImages: Set<String> = []
    var failure: String?

    @ObservationIgnored private(set) var client: DockerClient?
    /// Where to find Docker; nil while no engine is available.
    @ObservationIgnored var socketPath: @MainActor () -> String? = { DockerClient.discoverExternal()?.socketPath }
    /// Called when Docker deletes something, whoever asked it to, so the space can go back to the Mac.
    @ObservationIgnored var onSpaceFreed: @MainActor () -> Void = {}
    /// Called with the host ports running containers publish.
    @ObservationIgnored var onPortsChanged: @MainActor (Set<UInt16>) -> Void = { _ in }
    @ObservationIgnored private var live: [String: LiveStats] = [:]
    @ObservationIgnored private var statsTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var refreshTask: Task<Void, Never>?
    @ObservationIgnored private var lastPrune = Date.distantPast

    // MARK: Lifecycle

    /// Connects, then follows the event stream forever, reconnecting when Docker goes away.
    func run() async {
        while !Task.isCancelled {
            do {
                guard let path = socketPath() else { throw DockerError(message: "Docker engine isn't running") }
                let client = DockerClient(socketPath: path)
                self.client = client
                let version = try await client.version()
                // Load containers before showing them, so "No containers yet" never flashes.
                try await refresh()
                await refreshImages()
                withAnimation(.smooth) {
                    connection = .connected(engine: version.Platform.map(\.Name).flatMap { $0.isEmpty ? nil : $0 } ?? "Docker \(version.Version)")
                }
                try await followEvents(client)
            } catch {
                withAnimation(.smooth) {
                    connection = .unavailable(error.localizedDescription)
                    containers = []
                    volumes = nil
                    images = nil
                }
                stopAllStats()
            }
            try? await Task.sleep(for: .seconds(1))
        }
    }

    private func followEvents(_ client: DockerClient) async throws {
        // Docker sends an event per state change; refresh on each, debounced.
        let periodic = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(15))
                try? await self?.refresh()
            }
        }
        defer { periodic.cancel() }
        var lines = LineSplitter()
        for try await chunk in client.events() {
            scheduleRefresh()
            if lines.feed(chunk).contains(where: Self.freesSpace) { onSpaceFreed() }
        }
    }

    /// Whether a Docker event deleted something that took disk space.
    private static func freesSpace(_ line: Data) -> Bool {
        guard let event = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
              let type = event["Type"] as? String, let action = event["Action"] as? String else { return false }
        switch (type, action) {
        case ("container", "destroy"), ("image", "delete"), ("volume", "destroy"), ("builder", "prune"): return true
        default: return false
        }
    }

    private func scheduleRefresh() {
        refreshTask?.cancel()
        refreshTask = Task {
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            try? await refresh()
            // Only on events, not the periodic refresh: measuring walks every volume's files.
            await refreshVolumes()
            await refreshImages()
        }
    }

    func refresh() async throws {
        guard let client else { return }
        let fresh = try await client.containers().sorted {
            ($0.project ?? "~", $0.name) < ($1.project ?? "~", $1.name)
        }
        if fresh != containers {
            withAnimation(.snappy) { containers = fresh }
        }
        syncStats()
        if Date.now.timeIntervalSince(lastPrune) > 600 {
            lastPrune = .now
            Task { await ArchiveRetention.prune(fresh, client: client) }
        }
        onPortsChanged(Set(fresh.filter(\.isRunning).flatMap(\.publishedPorts).compactMap { UInt16(exactly: $0) }))
    }

    // MARK: Volumes

    /// Sizing a volume means walking its files, so volumes are only loaded while
    /// the list is on screen, and then again on Docker events rather than a timer.
    var wantsVolumes = false {
        didSet { if wantsVolumes, !oldValue { Task { await refreshVolumes() } } }
    }

    func refreshVolumes() async {
        guard wantsVolumes, let client, let fresh = try? await client.volumes() else { return }
        let sorted = fresh.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        if sorted != volumes {
            withAnimation(.snappy) { volumes = sorted }
        }
    }

    /// Every container that mounts the volume, running or not.
    func containers(using volume: Volume) -> [Container] {
        containers.filter { $0.volumes.contains(volume.name) }
    }

    /// Removes each volume by name, so only the ones the person confirmed go, and
    /// Docker keeps any a container started using in the meantime.
    func removeVolumes(_ names: [String]) {
        guard let client, !names.isEmpty else { return }
        withAnimation(.snappy) { removingVolumes.formUnion(names) }
        Task {
            var failed: [String] = []
            for name in names {
                do { try await client.removeVolume(name) } catch { failed.append(name) }
            }
            if !failed.isEmpty {
                failure = failed.count == 1
                    ? "Couldn't remove \(failed[0]). A container may have started using it."
                    : "Couldn't remove \(failed.count) volumes. Containers may have started using them."
            }
            await refreshVolumes()
            withAnimation(.snappy) { removingVolumes.subtract(names) }
        }
    }

    // MARK: Actions

    func perform(_ action: ContainerAction, on container: Container) {
        guard let client, busy[container.id] == nil else { return }
        if action == .start || action == .restart { Archive.setArchived(container.id, nil) }
        withAnimation(.snappy) { busy[container.id] = action }
        Task {
            do {
                try await client.perform(action, on: container.id)
            } catch {
                failure = error.localizedDescription
            }
            try? await refresh()
            withAnimation(.snappy) { busy[container.id] = nil }
        }
    }

    /// Removes containers, and optionally their volumes: the ones they mount, plus a
    /// Compose project's own named volumes. Volumes go only after the containers are
    /// gone and without force, so Docker keeps any that another container still uses.
    func remove(_ targets: [Container], deletingVolumes: Bool, project: String? = nil) {
        guard let client, !targets.isEmpty else { return }
        withAnimation(.snappy) { for container in targets { busy[container.id] = .remove } }
        Task {
            let volumes = deletingVolumes ? await Self.volumes(of: targets, project: project, client: client) : []
            for container in targets {
                do {
                    try await client.perform(.remove, on: container.id)
                    Archive.remove(container.id)
                } catch {
                    failure = error.localizedDescription
                }
            }
            for volume in volumes {
                try? await client.removeVolume(volume)
            }
            try? await refresh()
            withAnimation(.snappy) { for container in targets { busy[container.id] = nil } }
        }
    }

    private static func volumes(of containers: [Container], project: String?, client: DockerClient) async -> Set<String> {
        var names = Set<String>()
        for container in containers {
            guard let data = try? await client.send("GET", "/containers/\(container.id)/json"),
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let mounts = json["Mounts"] as? [[String: Any]] else { continue }
            for mount in mounts where mount["Type"] as? String == "volume" {
                if let name = mount["Name"] as? String { names.insert(name) }
            }
        }
        if let project {
            let filter = #"{"label":["com.docker.compose.project=\#(project)"]}"#.addingPercentEncoding(withAllowedCharacters: .alphanumerics)!
            if let data = try? await client.send("GET", "/volumes?filters=\(filter)"),
               let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let volumes = json["Volumes"] as? [[String: Any]] {
                names.formUnion(volumes.compactMap { $0["Name"] as? String })
            }
        }
        return names
    }

    /// Stops a container and keeps it; it's deleted after the retention period.
    func archive(_ container: Container) {
        Archive.setArchived(container.id, .now)
        if container.isRunning { perform(.stop, on: container) } else { Task { try? await refresh() } }
    }

    func restore(_ container: Container) {
        Archive.setArchived(container.id, nil)
        if container.isRunning { Task { try? await refresh() } } else { perform(.start, on: container) }
    }

    func perform(_ action: ContainerAction, onProject project: String) {
        for container in containers where container.project == project {
            let applies = switch action {
            case .remove: true
            case .stop, .restart: container.isRunning
            default: !container.isRunning
            }
            if applies { perform(action, on: container) }
        }
    }

    // MARK: Images

    func refreshImages() async {
        guard let client, let fresh = try? await client.images() else { return }
        rememberTags(fresh)
        // Tagged by name, then untagged newest first.
        let sorted = fresh.sorted { a, b in
            if a.isUntagged != b.isUntagged { return !a.isUntagged }
            return a.isUntagged ? a.created > b.created : a.tags[0].localizedStandardCompare(b.tags[0]) == .orderedAscending
        }
        if sorted != images {
            withAnimation(.snappy) { images = sorted }
        }
    }

    /// The tags an image had before they moved to newer images, as far as Kuvo saw.
    /// Docker keeps nothing about an untagged image's name, so Kuvo remembers it,
    /// across launches, for images that still exist.
    func formerTags(of image: DockerImage) -> [String] {
        image.isUntagged ? knownTags[image.id] ?? [] : []
    }

    @ObservationIgnored private lazy var knownTags: [String: [String]] =
        UserDefaults.standard.dictionary(forKey: Self.knownTagsKey) as? [String: [String]] ?? [:]
    private static let knownTagsKey = "images.knownTags"

    private func rememberTags(_ images: [DockerImage]) {
        var known = knownTags.filter { id, _ in images.contains { $0.id == id } }
        for image in images where !image.isUntagged { known[image.id] = image.tags }
        guard known != knownTags else { return }
        knownTags = known
        UserDefaults.standard.set(known, forKey: Self.knownTagsKey)
    }

    /// Every container created from the image, running or not.
    func containers(using image: DockerImage) -> [Container] {
        containers.filter { $0.imageID == image.id }
    }

    /// Removes each image tag by tag, so the image goes with its last tag, and an
    /// untagged one by ID. Never forced: Docker keeps any a container uses.
    func removeImages(_ targets: [DockerImage]) {
        guard !targets.isEmpty else { return }
        Task {
            let failed = await deleteImages(targets)
            if !failed.isEmpty {
                failure = failed.count == 1
                    ? "Couldn't remove \(failed[0]). A container may have started using it."
                    : "Couldn't remove \(failed.count) images. Containers may have started using them."
            }
        }
    }

    /// Removes each image tag by tag, then an untagged one by ID, and returns the names
    /// of those that couldn't go. An image a container runs is never forced, so Docker
    /// keeps it. One no container runs is: Docker would otherwise keep it for a container
    /// created from one of its tags that now runs an older image (as after a pull), and
    /// untagging doesn't affect that container, which runs its image by ID.
    @discardableResult
    func deleteImages(_ targets: [DockerImage]) async -> [String] {
        guard let client else { return targets.map { $0.tags.first ?? $0.shortID } }
        withAnimation(.snappy) { removingImages.formUnion(targets.map(\.id)) }
        var failed: [String] = []
        for image in targets {
            let unused = containers(using: image).isEmpty
            do {
                for reference in image.isUntagged ? [image.id] : image.tags {
                    try await client.removeImage(reference, force: unused)
                }
            } catch {
                failed.append(image.tags.first ?? image.shortID)
            }
        }
        await refreshImages()
        withAnimation(.snappy) { removingImages.subtract(targets.map(\.id)) }
        return failed
    }

    /// Images no container runs: what the Images list calls unused.
    var unusedImages: [DockerImage] { (images ?? []).filter { containers(using: $0).isEmpty } }

    /// Removes one of an image's several tags; the image stays with the others.
    func removeTag(_ tag: String) {
        guard let client else { return }
        Task {
            do { try await client.removeImage(tag) } catch { failure = error.localizedDescription }
            await refreshImages()
        }
    }

    /// Pulls in progress, shown at the top of the image list.
    private(set) var pulls: [ImagePull] = []
    /// The image a pull just finished with, for the list to select.
    private(set) var pulledImageID: String?
    /// Progress updates hop to the main actor on their own, so one can land after its pull ended.
    @ObservationIgnored private var activePulls: Set<String> = []

    /// Starts pulling the image's current version, returning once the download has
    /// begun, or once it's done if there's nothing new. Errors that come before any
    /// download (no such image, a login needed, a bad name) are thrown here, so the
    /// sheet that asked can show them; later ones go to `failure`.
    func startPull(_ name: String) async throws {
        guard let client else { throw DockerError(message: "Docker engine isn't running") }
        guard !activePulls.contains(name) else { throw DockerError(message: "\(name) is already being pulled.") }
        let started = PullStart()
        activePulls.insert(name)
        pulledImageID = nil
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            started.continuation = continuation
            Task {
                do {
                    let outcome = try await client.pullLatest(name) { progress in
                        Task { @MainActor in
                            started.resume()
                            if self.activePulls.contains(name) { self.show(ImagePull(name: name, progress: progress)) }
                        }
                    }
                    started.resume()
                    await finishPull(name, outcome: outcome, client: client)
                } catch {
                    activePulls.remove(name)
                    if !started.resume(throwing: error) { failure = error.localizedDescription }
                    withAnimation(.snappy) { pulls.removeAll { $0.name == name } }
                }
            }
        }
    }

    private func show(_ pull: ImagePull) {
        withAnimation(.snappy) {
            if let index = pulls.firstIndex(where: { $0.name == pull.name }) { pulls[index] = pull } else { pulls.insert(pull, at: 0) }
        }
    }

    private func finishPull(_ name: String, outcome: PullOutcome, client: DockerClient) async {
        activePulls.remove(name)
        await refreshImages()
        let id = try? await client.inspectImage(name).Id
        withAnimation(.snappy) { pulledImageID = id }
        if outcome == .upToDate {
            // Say so briefly where the progress was, then let the image's own row speak.
            show(ImagePull(name: name, progress: PullProgress(), upToDate: true))
            try? await Task.sleep(for: .seconds(3))
        }
        withAnimation(.snappy) { pulls.removeAll { $0.name == name } }
    }

    // MARK: Stats

    func stats(for id: String) -> LiveStats {
        if let existing = live[id] { return existing }
        let stats = LiveStats()
        live[id] = stats
        return stats
    }

    /// Live stats are only streamed while someone can see them.
    var wantsLiveStats = true {
        didSet { if oldValue != wantsLiveStats { syncStats() } }
    }

    private func syncStats() {
        let running = wantsLiveStats ? Set(containers.filter(\.isRunning).map(\.id)) : []
        for (id, task) in statsTasks where !running.contains(id) {
            task.cancel()
            statsTasks[id] = nil
            live[id]?.reset()
        }
        for id in running where statsTasks[id] == nil {
            statsTasks[id] = streamStats(id)
        }
        live = live.filter { id, _ in containers.contains { $0.id == id } }
    }

    private func streamStats(_ id: String) -> Task<Void, Never> {
        guard let client else { return Task {} }
        let stats = stats(for: id)
        return Task.detached {
            var lines = LineSplitter()
            let decoder = JSONDecoder()
            do {
                for try await chunk in client.stats(id) {
                    for line in lines.feed(chunk) {
                        guard let sample = try? decoder.decode(StatsSample.self, from: line) else { continue }
                        await stats.push(sample)
                    }
                }
            } catch {}
        }
    }

    private func stopAllStats() {
        statsTasks.values.forEach { $0.cancel() }
        statsTasks = [:]
        live = [:]
    }
}

/// An image being pulled, as its row in the list shows it.
struct ImagePull: Identifiable, Equatable {
    let name: String
    var progress: PullProgress
    var upToDate = false
    var id: String { name }
}

/// Resumes a pull's caller exactly once: when the download starts, when it ends, or when it fails first.
@MainActor private final class PullStart {
    var continuation: CheckedContinuation<Void, Error>?

    func resume() {
        continuation?.resume()
        continuation = nil
    }

    /// Returns whether the caller was still waiting for this error.
    @discardableResult
    func resume(throwing error: Error) -> Bool {
        guard let continuation else { return false }
        continuation.resume(throwing: error)
        self.continuation = nil
        return true
    }
}

/// Rolling stats for one container. Each container gets its own observable
/// object so a new sample only invalidates the views showing that container.
@MainActor @Observable
final class LiveStats {
    static let window = 60

    private(set) var cpu: [Double] = []
    private(set) var memory: [Double] = []
    private(set) var network: [Double] = []
    private(set) var memoryLimit: Double = 0
    private(set) var rxRate: Double = 0
    private(set) var txRate: Double = 0
    private(set) var tick = 0

    @ObservationIgnored private var previous: (date: Date, rx: UInt64, tx: UInt64)?

    var hasData: Bool { tick > 0 }

    func push(_ sample: StatsSample) {
        guard sample.precpu_stats.system_cpu_usage != nil else { return }
        let now = Date()
        if let previous {
            let dt = max(now.timeIntervalSince(previous.date), 0.1)
            rxRate = Double(sample.rxBytes &- previous.rx) / dt
            txRate = Double(sample.txBytes &- previous.tx) / dt
        }
        previous = (now, sample.rxBytes, sample.txBytes)

        append(&cpu, sample.cpuPercent)
        append(&memory, Double(sample.memoryUsed))
        append(&network, rxRate + txRate)
        memoryLimit = Double(sample.memoryLimit)
        tick += 1
    }

    func reset() {
        cpu = []; memory = []; network = []
        rxRate = 0; txRate = 0; previous = nil
    }

    private func append(_ series: inout [Double], _ value: Double) {
        series.append(value.isFinite ? max(value, 0) : 0)
        // One extra point so the oldest sample can slide out of view smoothly.
        if series.count > Self.window + 1 { series.removeFirst(series.count - Self.window - 1) }
    }
}
