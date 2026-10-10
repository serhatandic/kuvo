import Foundation
import SwiftUI

/// Terrarium's own Docker engine: a small Alpine VM running dockerd,
/// exposed on `~/.terrarium/docker.sock`.
@MainActor @Observable
final class Engine {
    enum State: Equatable {
        case stopped
        case downloading(Double)
        case booting(String)
        case running
        case stopping
        case failed(String)
    }

    private(set) var state = State.stopped

    @ObservationIgnored private var vm: VirtualMachine?
    @ObservationIgnored private var bridge: SocketBridge?
    @ObservationIgnored private var forwards: [UInt16: SocketBridge] = [:]
    @ObservationIgnored @AppStorage("engine.installed") private var installed = false

    var socketPath: String? { state == .running ? EnginePaths.socket.path : nil }

    func start() async {
        switch state {
        case .stopped, .failed: break
        default: return
        }
        do {
            if !GuestImage.isDownloaded { state = .downloading(0) }
            try await GuestImage.prepare { [weak self] progress in
                Task { @MainActor in
                    if case .downloading = self?.state { self?.state = .downloading(progress) }
                }
            }
            withAnimation(.smooth) { state = .booting(installed ? "Starting" : "Starting Linux") }

            let memory = min(ProcessInfo.processInfo.physicalMemory / 2, 8 << 30)
            let vm = try VirtualMachine(cpus: max(2, ProcessInfo.processInfo.activeProcessorCount / 2), memory: memory)
            vm.onStep = { [weak self] step in self?.guestReported(step) }
            vm.onStop = { [weak self] error in self?.vmStopped(error) }
            try await vm.start()
            self.vm = vm

            bridge = try SocketBridge(unixPath: EnginePaths.socket.path) { [vm] in
                try await vm.connect(port: GuestScripts.dockerPort)
            }
            try await waitForDocker(timeout: installed ? .seconds(90) : .seconds(900))
            installed = true
            withAnimation(.smooth) { state = .running }
        } catch {
            await teardown()
            withAnimation(.smooth) { state = .failed(error.localizedDescription) }
        }
    }

    func stop() async {
        guard state != .stopped, state != .stopping else { return }
        withAnimation(.smooth) { state = .stopping }
        await teardown()
        withAnimation(.smooth) { state = .stopped }
    }

    func restart() async {
        await stop()
        await start()
    }

    /// Mirrors published container ports onto localhost.
    func forward(_ ports: Set<UInt16>) {
        for (port, bridge) in forwards where !ports.contains(port) {
            bridge.stop()
            forwards[port] = nil
        }
        guard state == .running,
              let ip = try? String(contentsOf: EnginePaths.share.appending(path: "ip"), encoding: .utf8)
                .trimmingCharacters(in: .whitespacesAndNewlines), !ip.isEmpty
        else { return }

        for port in ports where forwards[port] == nil {
            forwards[port] = try? SocketBridge(tcpPort: port) { try connectTCP(host: ip, port: port) }
        }
    }

    // MARK: Private

    private func guestReported(_ step: String) {
        if step.hasPrefix("error: ") {
            withAnimation(.smooth) { state = .failed(String(step.dropFirst(7)).capitalizedSentence) }
        } else if case .booting = state {
            withAnimation(.smooth) { state = .booting(step) }
        }
    }

    private func vmStopped(_ error: Error?) {
        guard state != .stopping else { return }
        Task { await teardown() }
        if case .failed = state { return }
        withAnimation(.smooth) {
            state = error.map { .failed($0.localizedDescription) } ?? .stopped
        }
    }

    private func waitForDocker(timeout: Duration) async throws {
        let client = DockerClient(socketPath: EnginePaths.socket.path)
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            if case .failed(let message) = state { throw DockerError(message: message) }
            if (try? await client.ping()) != nil { return }
            try await Task.sleep(for: .milliseconds(400))
        }
        throw DockerError(message: "Docker didn't start in time. See console.log for details.")
    }

    private func teardown() async {
        forward([])
        bridge?.stop()
        bridge = nil
        unlink(EnginePaths.socket.path)
        await vm?.stop()
        vm = nil
    }
}

/// Blocking TCP connect; runs off the main actor inside the bridge.
private func connectTCP(host: String, port: UInt16) throws -> Int32 {
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    var address = sockaddr_in()
    address.sin_family = sa_family_t(AF_INET)
    address.sin_port = port.bigEndian
    address.sin_addr.s_addr = inet_addr(host)
    let result = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) }
    }
    guard result == 0 else {
        close(fd)
        throw DockerError(message: "Couldn't reach port \(port) in the VM")
    }
    return fd
}

private extension String {
    var capitalizedSentence: String { prefix(1).uppercased() + dropFirst() }
}
