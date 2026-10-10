import CryptoKit
import Foundation

/// Where the engine keeps its files.
enum EnginePaths {
    static let support = URL.applicationSupportDirectory.appending(path: "Terrarium", directoryHint: .isDirectory)
    static let downloads = support.appending(path: "downloads", directoryHint: .isDirectory)
    /// Shared into the guest over virtiofs as `terrarium`.
    static let share = support.appending(path: "share", directoryHint: .isDirectory)
    static let kernel = support.appending(path: "kernel")
    static let initramfs = support.appending(path: "initramfs")
    static let disk = support.appending(path: "disk.img")
    static let console = support.appending(path: "console.log")
    static let macAddress = support.appending(path: "mac-address")
    static let socket = URL.homeDirectory.appending(path: ".terrarium/docker.sock")
}

/// Fetches Alpine Linux and turns it into something Virtualization.framework can boot.
enum GuestImage {
    struct Asset {
        let name: String
        let url: String
        let sha256: String
        let size: Int64
    }

    static let alpine = "3.24.2"
    private static let base = "https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64"

    static let assets = [
        Asset(name: "vmlinuz-virt", url: "\(base)/netboot-\(alpine)/vmlinuz-virt",
              sha256: "e45e1f6083d1ed45db6647b422e32b6ae6dc54de7b8190b7b97744fb293412e3", size: 10_387_968),
        Asset(name: "initramfs-virt", url: "\(base)/netboot-\(alpine)/initramfs-virt",
              sha256: "ffe65ec5a0c0bf470042ad28f7ce7aa5f842ce8090e4230fb2703a7a34e1bebe", size: 9_399_083),
        Asset(name: "modloop-virt", url: "\(base)/netboot-\(alpine)/modloop-virt",
              sha256: "32a189d5e4ae4417cf65bfcc91d2cc8ffdf5957fbbc1f055b037861c595b7328", size: 17_743_872),
        Asset(name: "rootfs.tar.gz", url: "\(base)/alpine-minirootfs-\(alpine)-aarch64.tar.gz",
              sha256: "9bf70a7f18ea44094cbb5f70c58f9af129c8214745743db0e68e5502cc2ce773", size: 4_028_030),
    ]

    static var isDownloaded: Bool {
        assets.allSatisfy { FileManager.default.fileExists(atPath: EnginePaths.downloads.appending(path: $0.name).path) }
    }

    /// Downloads (if needed) and lays out everything the VM needs to boot.
    static func prepare(progress: @escaping @Sendable (Double) -> Void) async throws {
        let fm = FileManager.default
        for dir in [EnginePaths.downloads, EnginePaths.share, EnginePaths.socket.deletingLastPathComponent()] {
            try fm.createDirectory(at: dir, withIntermediateDirectories: true)
        }

        let total = Double(assets.reduce(0) { $0 + $1.size })
        var done: Int64 = 0
        for asset in assets {
            let target = EnginePaths.downloads.appending(path: asset.name)
            if !fm.fileExists(atPath: target.path) {
                let offset = done
                try await download(asset, to: target) { bytes in progress(Double(offset + bytes) / total) }
            }
            done += asset.size
            progress(Double(done) / total)
        }

        try unwrapKernel(EnginePaths.downloads.appending(path: "vmlinuz-virt"), to: EnginePaths.kernel)
        try buildInitramfs(EnginePaths.downloads.appending(path: "initramfs-virt"), to: EnginePaths.initramfs)
        try link(EnginePaths.downloads.appending(path: "modloop-virt"), to: EnginePaths.share.appending(path: "modloop"))
        try link(EnginePaths.downloads.appending(path: "rootfs.tar.gz"), to: EnginePaths.share.appending(path: "rootfs.tar.gz"))
        for (name, text) in [("inittab", GuestScripts.inittab), ("boot.sh", GuestScripts.boot), ("shutdown.sh", GuestScripts.shutdown)] {
            try Data(text.utf8).write(to: EnginePaths.share.appending(path: name), options: .atomic)
        }

        if !fm.fileExists(atPath: EnginePaths.disk.path) {
            // Sparse: takes no space until written. ext4 inside grows into it.
            fm.createFile(atPath: EnginePaths.disk.path, contents: nil)
            let handle = try FileHandle(forWritingTo: EnginePaths.disk)
            try handle.truncate(atOffset: 64 << 30)
            try handle.close()
        }
    }

    // MARK: Steps

    private static func download(_ asset: Asset, to target: URL, progress: @escaping @Sendable (Int64) -> Void) async throws {
        let (temp, response) = try await URLSession.shared.download(
            from: URL(string: asset.url)!, delegate: DownloadProgress(report: progress)
        )
        guard (response as? HTTPURLResponse)?.statusCode == 200 else {
            throw DockerError(message: "Couldn't download \(asset.name)")
        }
        guard try sha256(of: temp) == asset.sha256 else {
            try? FileManager.default.removeItem(at: temp)
            throw DockerError(message: "Checksum mismatch for \(asset.name)")
        }
        try? FileManager.default.removeItem(at: target)
        try FileManager.default.moveItem(at: temp, to: target)
    }

    private static func sha256(of url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hasher = SHA256()
        while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty { hasher.update(data: chunk) }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    /// Alpine's arm64 kernel is an EFI "zboot" image: a PE stub wrapping a gzip
    /// payload. The Linux boot loader wants the raw `Image`, so unwrap it.
    static func unwrapKernel(_ source: URL, to target: URL) throws {
        let data = try Data(contentsOf: source)
        guard data.count > 64, data[4..<8] == Data("zimg".utf8) else {
            return try link(source, to: target) // Already a raw image.
        }
        let offset = Int(data.loadLE(UInt32.self, at: 8))
        let size = Int(data.loadLE(UInt32.self, at: 12))
        let kernel = try gunzip(data.subdata(in: offset..<offset + size))
        try kernel.write(to: target, options: .atomic)
    }

    /// Alpine's initramfs with our `/init` appended. The kernel unpacks
    /// concatenated archives in order, so later files replace earlier ones.
    static func buildInitramfs(_ source: URL, to target: URL) throws {
        var data = try Data(contentsOf: source)
        data.append(Data(count: (4 - data.count % 4) % 4)) // Archives must start 4-byte aligned.
        data.append(CPIO.archive([.init(path: "init", mode: 0o100755, contents: Data(GuestScripts.stage1.utf8))]))
        try data.write(to: target, options: .atomic)
    }

    private static func gunzip(_ data: Data) throws -> Data {
        // Skip the gzip header (RFC 1952) to reach the raw DEFLATE stream.
        guard data.count > 18, data[0] == 0x1F, data[1] == 0x8B else { throw DockerError(message: "Kernel isn't gzip") }
        let flags = data[3]
        var i = 10
        if flags & 0x04 != 0 { i += 2 + Int(data[i]) | Int(data[i + 1]) << 8 }
        if flags & 0x08 != 0 { i = (data[i...].firstIndex(of: 0) ?? i) + 1 }
        if flags & 0x10 != 0 { i = (data[i...].firstIndex(of: 0) ?? i) + 1 }
        if flags & 0x02 != 0 { i += 2 }
        return try (data.subdata(in: i..<data.count - 8) as NSData).decompressed(using: .zlib) as Data
    }

    private static func link(_ source: URL, to target: URL) throws {
        try? FileManager.default.removeItem(at: target)
        try FileManager.default.linkItem(at: source, to: target)
    }
}

private final class DownloadProgress: NSObject, URLSessionDownloadDelegate, Sendable {
    let report: @Sendable (Int64) -> Void
    init(report: @escaping @Sendable (Int64) -> Void) { self.report = report }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData _: Int64,
                    totalBytesWritten: Int64, totalBytesExpectedToWrite _: Int64) {
        report(totalBytesWritten)
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {}
}

/// Just enough of the `newc` cpio format to drop files into an initramfs.
enum CPIO {
    struct File {
        let path: String
        let mode: UInt32
        let contents: Data
    }

    static func archive(_ files: [File]) -> Data {
        var out = Data()
        for (i, file) in files.enumerated() {
            entry(&out, ino: UInt32(i + 1), mode: file.mode, name: file.path, contents: file.contents)
        }
        entry(&out, ino: 0, mode: 0, name: "TRAILER!!!", contents: Data())
        return out
    }

    private static func entry(_ out: inout Data, ino: UInt32, mode: UInt32, name: String, contents: Data) {
        let fields: [UInt32] = [ino, mode, 0, 0, 1, 0, UInt32(contents.count), 0, 0, 0, 0, UInt32(name.utf8.count + 1), 0]
        out.append(Data(("070701" + fields.map { String(format: "%08X", $0) }.joined()).utf8))
        out.append(Data(name.utf8) + [0])
        out.append(Data(count: (4 - out.count % 4) % 4))
        out.append(contents)
        out.append(Data(count: (4 - out.count % 4) % 4))
    }
}

private extension Data {
    func loadLE<T: FixedWidthInteger>(_: T.Type, at offset: Int) -> T {
        subdata(in: offset..<offset + MemoryLayout<T>.size).withUnsafeBytes { T(littleEndian: $0.loadUnaligned(as: T.self)) }
    }
}
