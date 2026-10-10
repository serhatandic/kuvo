import Foundation

/// `terrarium shell`: an interactive TTY inside a sandbox. Docker hands over
/// the raw connection after an HTTP upgrade; we put the terminal in raw mode
/// and copy bytes both ways, resizing along with the window.
enum Shell {
    static func run(_ sandbox: Container, client: DockerClient) async throws -> Int32 {
        guard isatty(STDIN_FILENO) == 1 else {
            throw DockerError(message: "shell needs a terminal. Use terrarium exec to run commands.")
        }
        struct Created: Decodable { let Id: String }
        let term = ProcessInfo.processInfo.environment["TERM"] ?? "xterm-256color"
        let exec = try await client.send("POST", "/containers/\(sandbox.id)/exec", json: [
            "AttachStdin": true, "AttachStdout": true, "AttachStderr": true, "Tty": true,
            "Env": ["TERM=\(term)"], "WorkingDir": Sandboxes.workspace,
            "Cmd": ["sh", "-c", "command -v bash >/dev/null && exec bash -l || exec sh"],
        ], as: Created.self)

        let socket = try open(client.socketPath)
        defer { close(socket) }
        let body = #"{"Detach":false,"Tty":true}"#
        let request = "POST /exec/\(exec.Id)/start HTTP/1.1\r\nHost: docker\r\nConnection: Upgrade\r\nUpgrade: tcp\r\n"
            + "Content-Type: application/json\r\nContent-Length: \(body.utf8.count)\r\n\r\n\(body)"
        guard writeAll(socket, Array(request.utf8)) else { throw DockerError(message: "Couldn't start the shell") }
        let leftover = try readHead(socket)

        // Raw mode: keystrokes go straight to the remote shell.
        var original = termios()
        tcgetattr(STDIN_FILENO, &original)
        var raw = original
        cfmakeraw(&raw)
        tcsetattr(STDIN_FILENO, TCSANOW, &raw)
        defer { tcsetattr(STDIN_FILENO, TCSANOW, &original) }

        let resize = { @Sendable in
            var size = winsize()
            guard ioctl(STDOUT_FILENO, 0x4008_7468 /* TIOCGWINSZ */, &size) == 0 else { return }
            Task { _ = try? await client.send("POST", "/exec/\(exec.Id)/resize?h=\(size.ws_row)&w=\(size.ws_col)") }
        }
        resize()
        signal(SIGWINCH, SIG_IGN)
        let winch = DispatchSource.makeSignalSource(signal: SIGWINCH, queue: .main)
        winch.setEventHandler(handler: resize)
        winch.resume()
        defer { winch.cancel() }

        if !leftover.isEmpty { _ = writeAll(STDOUT_FILENO, leftover) }
        Thread.detachNewThread { pump(from: STDIN_FILENO, to: socket) }
        await withCheckedContinuation { continuation in
            Thread.detachNewThread {
                pump(from: socket, to: STDOUT_FILENO)
                continuation.resume()
            }
        }

        struct Inspect: Decodable { let ExitCode: Int? }
        let state = try? await JSONDecoder().decode(Inspect.self, from: client.send("GET", "/exec/\(exec.Id)/json"))
        return Int32(truncatingIfNeeded: state?.ExitCode ?? 0)
    }

    private static func open(_ path: String) throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: path.utf8.prefix($0.count - 1)) }
        let connected = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard fd >= 0, connected == 0 else { throw DockerError(message: "Couldn't connect to Terrarium") }
        return fd
    }

    /// Reads the HTTP response head and returns any stream bytes that came with it.
    private static func readHead(_ fd: Int32) throws -> [UInt8] {
        var buffer: [UInt8] = []
        var chunk = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = read(fd, &chunk, chunk.count)
            guard n > 0 else { throw DockerError(message: "The shell closed before starting") }
            buffer += chunk[..<n]
            if let end = buffer.firstRange(of: [13, 10, 13, 10]) {
                let status = String(decoding: buffer[..<end.lowerBound], as: UTF8.self)
                guard status.hasPrefix("HTTP/1.1 101") || status.hasPrefix("HTTP/1.1 200") else {
                    throw DockerError(message: "Docker refused the shell: \(status.prefix(80))")
                }
                return Array(buffer[end.upperBound...])
            }
        }
    }

    private static func pump(from source: Int32, to destination: Int32) {
        var buffer = [UInt8](repeating: 0, count: 16 * 1024)
        while true {
            let n = read(source, &buffer, buffer.count)
            if n < 0 && errno == EINTR { continue }
            guard n > 0, writeAll(destination, Array(buffer[..<n])) else { break }
        }
        if destination != STDOUT_FILENO { shutdown(destination, SHUT_WR) }
    }

    private static func writeAll(_ fd: Int32, _ bytes: [UInt8]) -> Bool {
        var offset = 0
        while offset < bytes.count {
            let n = bytes.withUnsafeBytes { write(fd, $0.baseAddress! + offset, bytes.count - offset) }
            if n < 0 && errno == EINTR { continue }
            guard n > 0 else { return false }
            offset += n
        }
        return true
    }
}
