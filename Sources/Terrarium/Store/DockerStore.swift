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
    var failure: String?

    @ObservationIgnored private(set) var client: DockerClient?
    /// Where to find Docker; nil while no engine is available.
    @ObservationIgnored var socketPath: @MainActor () -> String? = { DockerClient.discoverExternal()?.socketPath }
    /// Called with the host ports running containers publish.
    @ObservationIgnored var onPortsChanged: @MainActor (Set<UInt16>) -> Void = { _ in }
    @ObservationIgnored private var live: [String: LiveStats] = [:]
    @ObservationIgnored private var statsTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var refreshTask: Task<Void, Never>?

    // MARK: Lifecycle

    /// Connects, then follows the event stream forever, reconnecting when Docker goes away.
    func run() async {
        while !Task.isCancelled {
            do {
                guard let path = socketPath() else { throw DockerError(message: "Docker engine isn't running") }
                let client = DockerClient(socketPath: path)
                self.client = client
                let version = try await client.version()
                withAnimation(.smooth) {
                    connection = .connected(engine: version.Platform.map(\.Name).flatMap { $0.isEmpty ? nil : $0 } ?? "Docker \(version.Version)")
                }
                try await refresh()
                try await followEvents(client)
            } catch {
                withAnimation(.smooth) {
                    connection = .unavailable(error.localizedDescription)
                    containers = []
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
        for try await _ in client.events() { scheduleRefresh() }
    }

    private func scheduleRefresh() {
        refreshTask?.cancel()
        refreshTask = Task {
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            try? await refresh()
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
        onPortsChanged(Set(fresh.filter(\.isRunning).flatMap(\.publishedPorts).compactMap { UInt16(exactly: $0) }))
    }

    // MARK: Actions

    func perform(_ action: ContainerAction, on container: Container) {
        guard let client, busy[container.id] == nil else { return }
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

    func perform(_ action: ContainerAction, onProject project: String) {
        for container in containers where container.project == project {
            let applies = action == .stop || action == .restart ? container.isRunning : !container.isRunning
            if applies { perform(action, on: container) }
        }
    }

    // MARK: Stats

    func stats(for id: String) -> LiveStats {
        if let existing = live[id] { return existing }
        let stats = LiveStats()
        live[id] = stats
        return stats
    }

    private func syncStats() {
        let running = Set(containers.filter(\.isRunning).map(\.id))
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
