import Foundation

/// Accepts connections on a local socket and splices each one to a freshly
/// opened file descriptor (a vsock into the VM, or a TCP socket).
final class SocketBridge: @unchecked Sendable {
    typealias Connect = @Sendable () async throws -> Int32

    private let listener: Int32
    private let source: DispatchSourceRead
    private let connect: Connect

    /// Listens on a Unix socket at `path`.
    init(unixPath path: String, connect: @escaping Connect) throws {
        unlink(path)
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            buffer.copyBytes(from: path.utf8.prefix(buffer.count - 1))
        }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard fd >= 0, bound == 0, listen(fd, 128) == 0 else {
            close(fd)
            throw DockerError(message: "Couldn't listen on \(path): \(String(cString: strerror(errno)))")
        }
        listener = fd
        self.connect = connect
        source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .global())
        start()
    }

    /// Listens on 127.0.0.1:`port`.
    init(tcpPort port: UInt16, connect: @escaping Connect) throws {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        var yes: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, socklen_t(MemoryLayout<Int32>.size))
        var address = sockaddr_in()
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = port.bigEndian
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) }
        }
        guard fd >= 0, bound == 0, listen(fd, 128) == 0 else {
            close(fd)
            throw DockerError(message: "Port \(port) is already in use")
        }
        listener = fd
        self.connect = connect
        source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .global())
        start()
    }

    private func start() {
        source.setEventHandler { [listener, connect] in
            let client = accept(listener, nil, nil)
            guard client >= 0 else { return }
            Task {
                guard let upstream = try? await connect() else {
                    close(client)
                    return
                }
                Splice(client, upstream)
            }
        }
        source.setCancelHandler { [listener] in close(listener) }
        source.resume()
    }

    func stop() { source.cancel() }
    deinit { source.cancel() }
}

/// Copies bytes both ways between two sockets until both sides finish.
private final class Splice: @unchecked Sendable {
    private let queue = DispatchQueue(label: "terrarium.splice")
    private var sources: [DispatchSourceRead] = []
    private var open = 2
    private let a: Int32, b: Int32

    @discardableResult
    init(_ a: Int32, _ b: Int32) {
        self.a = a
        self.b = b
        for fd in [a, b] {
            var yes: Int32 = 1
            setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &yes, socklen_t(MemoryLayout<Int32>.size))
        }
        queue.async { [self] in
            sources = [pump(from: a, to: b), pump(from: b, to: a)]
        }
    }

    private func pump(from src: Int32, to dst: Int32) -> DispatchSourceRead {
        let source = DispatchSource.makeReadSource(fileDescriptor: src, queue: queue)
        var buffer = [UInt8](repeating: 0, count: 1 << 16)
        source.setEventHandler { [self] in
            let n = read(src, &buffer, buffer.count)
            guard n > 0, writeAll(dst, buffer, n) else {
                // Half-close: let the other side finish sending its response.
                shutdown(dst, SHUT_WR)
                source.cancel()
                return
            }
        }
        source.setCancelHandler { [self] in
            open -= 1
            if open == 0 {
                sources.forEach { $0.cancel() }
                close(a)
                close(b)
                sources = [] // Breaks the retain cycle; the splice is done.
            }
        }
        source.resume()
        return source
    }

    private func writeAll(_ fd: Int32, _ buffer: [UInt8], _ count: Int) -> Bool {
        var offset = 0
        while offset < count {
            let n = buffer.withUnsafeBytes { write(fd, $0.baseAddress! + offset, count - offset) }
            if n < 0 && errno == EINTR { continue }
            guard n > 0 else { return false }
            offset += n
        }
        return true
    }
}
