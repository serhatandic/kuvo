import Foundation
import Virtualization

/// The Linux VM that runs dockerd. A thin wrapper over Virtualization.framework.
@MainActor
final class VirtualMachine: NSObject, VZVirtualMachineDelegate {
    private let vm: VZVirtualMachine
    private let console = Pipe()
    private var stopped: CheckedContinuation<Void, Never>?

    /// Lines the guest prints as `kuvo: <step>` while booting.
    var onStep: ((String) -> Void)?
    var onStop: ((Error?) -> Void)?

    init(cpus: Int, memory: UInt64) throws {
        let config = VZVirtualMachineConfiguration()
        let boot = VZLinuxBootLoader(kernelURL: EnginePaths.kernel)
        boot.initialRamdiskURL = EnginePaths.initramfs
        boot.commandLine = "console=hvc0 loglevel=4"
        config.bootLoader = boot
        config.cpuCount = min(max(cpus, VZVirtualMachineConfiguration.minimumAllowedCPUCount),
                              VZVirtualMachineConfiguration.maximumAllowedCPUCount)
        config.memorySize = min(max(memory, VZVirtualMachineConfiguration.minimumAllowedMemorySize),
                                VZVirtualMachineConfiguration.maximumAllowedMemorySize)

        let serial = VZVirtioConsoleDeviceSerialPortConfiguration()
        serial.attachment = VZFileHandleSerialPortAttachment(fileHandleForReading: nil, fileHandleForWriting: console.fileHandleForWriting)
        config.serialPorts = [serial]

        let disk = try VZDiskImageStorageDeviceAttachment(url: EnginePaths.disk, readOnly: false, cachingMode: .automatic, synchronizationMode: .fsync)
        config.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: disk)]

        let network = VZVirtioNetworkDeviceConfiguration()
        network.attachment = VZNATNetworkDeviceAttachment()
        network.macAddress = Self.persistentMACAddress()
        config.networkDevices = [network]

        config.directorySharingDevices = [
            Self.share("kuvo", EnginePaths.share),
            Self.share("users", URL(filePath: "/Users")),
        ]
        // Rosetta runs linux/amd64 images at near-native speed.
        if VZLinuxRosettaDirectoryShare.availability == .installed, let rosetta = try? VZLinuxRosettaDirectoryShare() {
            let device = VZVirtioFileSystemDeviceConfiguration(tag: "rosetta")
            device.share = rosetta
            config.directorySharingDevices.append(device)
        }
        config.socketDevices = [VZVirtioSocketDeviceConfiguration()]
        config.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
        config.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]
        try config.validate()

        vm = VZVirtualMachine(configuration: config)
        super.init()
        vm.delegate = self
        followConsole()
    }

    func start() async throws {
        try await vm.start()
    }

    /// Asks the guest to power off cleanly, forcing it after a timeout.
    func stop() async {
        guard vm.state == .running else { return }
        await withCheckedContinuation { continuation in
            stopped = continuation
            Task {
                _ = try? await connect(port: GuestScripts.powerOffPort)
                try? await Task.sleep(for: .seconds(15))
                if vm.state != .stopped { try? await vm.stop() }
                resumeStopped()
            }
        }
    }

    /// Opens a vsock connection to the guest and returns a file descriptor we own.
    func connect(port: UInt32) async throws -> Int32 {
        guard let device = vm.socketDevices.first as? VZVirtioSocketDevice else {
            throw DockerError(message: "VM has no socket device")
        }
        let connection = try await device.connect(toPort: port)
        let fd = dup(connection.fileDescriptor)
        connection.close()
        guard fd >= 0 else { throw DockerError(message: "Couldn't open guest socket") }
        return fd
    }

    // MARK: VZVirtualMachineDelegate

    nonisolated func guestDidStop(_ virtualMachine: VZVirtualMachine) {
        MainActor.assumeIsolated { finished(nil) }
    }

    nonisolated func virtualMachine(_ virtualMachine: VZVirtualMachine, didStopWithError error: Error) {
        MainActor.assumeIsolated { finished(error) }
    }

    private func finished(_ error: Error?) {
        resumeStopped()
        onStop?(error)
    }

    private func resumeStopped() {
        stopped?.resume()
        stopped = nil
    }

    // MARK: Helpers

    /// Mirrors the guest console to a log file and surfaces progress lines.
    private func followConsole() {
        FileManager.default.createFile(atPath: EnginePaths.console.path, contents: nil)
        let log = try? FileHandle(forWritingTo: EnginePaths.console)
        let lines = ConsoleLines() // Only touched from the handler's serial queue.
        console.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard !data.isEmpty else { handle.readabilityHandler = nil; return }
            try? log?.write(contentsOf: data)
            let steps = lines.splitter.feed(data)
                .map { String(decoding: $0, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines) }
                .filter { $0.hasPrefix("kuvo: ") }
                .map { String($0.dropFirst("kuvo: ".count)) }
            guard !steps.isEmpty else { return }
            Task { @MainActor in steps.forEach { self?.onStep?($0) } }
        }
    }

    private static func share(_ tag: String, _ url: URL) -> VZVirtioFileSystemDeviceConfiguration {
        let device = VZVirtioFileSystemDeviceConfiguration(tag: tag)
        device.share = VZSingleDirectoryShare(directory: VZSharedDirectory(url: url, readOnly: false))
        return device
    }

    /// A stable MAC keeps the same DHCP lease (and guest IP) across launches.
    private static func persistentMACAddress() -> VZMACAddress {
        if let saved = try? String(contentsOf: EnginePaths.macAddress, encoding: .utf8),
           let mac = VZMACAddress(string: saved.trimmingCharacters(in: .whitespacesAndNewlines)) {
            return mac
        }
        let mac = VZMACAddress.randomLocallyAdministered()
        try? mac.string.write(to: EnginePaths.macAddress, atomically: true, encoding: .utf8)
        return mac
    }
}

private final class ConsoleLines: @unchecked Sendable {
    var splitter = LineSplitter()
}
