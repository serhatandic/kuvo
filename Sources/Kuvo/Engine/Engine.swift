import Foundation
import SwiftUI

/// Kuvo's own Docker engine: a small Alpine VM running dockerd,
/// exposed on `~/.kuvo/docker.sock`.
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
    @ObservationIgnored private var trimTask: Task<Void, Never>?
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
            // The setting outlives the disk when someone deletes Kuvo's data folder;
            // that disk is installed again and needs the longer first-boot wait.
            if !FileManager.default.fileExists(atPath: EnginePaths.disk.path) { installed = false }
            withAnimation(.smooth) { state = .booting(installed ? "Starting" : "Starting Linux") }

            let vm = try await startMachine()
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

    /// Tells macOS which of the VM disk's blocks are free, so disk.img shrinks on the
    /// Mac. Deleting files in the VM frees space only inside it until this runs. The
    /// guest syncs first: ext4 can't trim blocks freed since its last journal commit.
    func trim() async {
        guard state == .running, let vm, let fd = try? await vm.connect(port: GuestScripts.trimPort) else { return }
        // The guest closes the connection when fstrim is done.
        await Task.detached {
            var buffer = [UInt8](repeating: 0, count: 64)
            while read(fd, &buffer, buffer.count) > 0 {}
            close(fd)
        }.value
    }

    /// Trims shortly after the last deletion, so a burst of them costs one trim.
    func scheduleTrim() {
        trimTask?.cancel()
        trimTask = Task {
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            await trim()
        }
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

    /// Boots the VM. Right after a previous Kuvo quits, its disk can stay locked
    /// for a moment, so a failed start is retried briefly before giving up.
    private func startMachine() async throws -> VirtualMachine {
        let memory = min(ProcessInfo.processInfo.physicalMemory / 2, 8 << 30)
        var attempt = 0
        while true {
            do {
                let vm = try VirtualMachine(cpus: max(2, ProcessInfo.processInfo.activeProcessorCount / 2), memory: memory)
                vm.onStep = { [weak self] step in self?.guestReported(step) }
                vm.onStop = { [weak self] error in self?.vmStopped(error) }
                try await vm.start()
                return vm
            } catch {
                attempt += 1
                guard attempt < 10 else { throw error }
                try await Task.sleep(for: .milliseconds(500))
            }
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

extension String {
    var capitalizedSentence: String { prefix(1).uppercased() + dropFirst() }
}
