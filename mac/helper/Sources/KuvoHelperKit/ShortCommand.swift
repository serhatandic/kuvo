#if os(macOS)
import Darwin
import Foundation
import os

/// A short command of macOS's own (`open`), run to its end with a time limit, for the helper: what
/// it said on stderr and how it ended. It reads nothing and its stdout goes nowhere. It learns the
/// end from the termination handler, and takes only what was said by then, so something the command
/// left running can't hold it up.
enum ShortCommand {
    struct Result: Sendable {
        var status: Int32
        var errorText: String
        var timedOut: Bool
        var succeeded: Bool { status == 0 && !timedOut }
    }

    private struct Said {
        var errors = Data()
        var timedOut = false
    }

    static func run(_ program: String, _ arguments: [String], limit: Duration) async throws -> Result {
        let process = Process()
        process.executableURL = URL(filePath: program)
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        let errors = Pipe()
        process.standardError = errors
        let reading = errors.fileHandleForReading
        let said = OSAllocatedUnfairLock(initialState: Said())
        reading.readabilityHandler = { handle in
            let data = handle.availableData
            if !data.isEmpty { said.withLock { $0.errors.append(data) } }
        }
        return try await withCheckedThrowingContinuation { continuation in
            let ended = DispatchGroup()
            ended.enter()
            process.terminationHandler = { _ in ended.leave() }
            do {
                try process.run()
            } catch {
                reading.readabilityHandler = nil
                return continuation.resume(throwing: error)
            }
            // Still running at its limit: told to stop, and a moment later, made to.
            let watch = DispatchWorkItem {
                guard process.isRunning else { return }
                said.withLock { $0.timedOut = true }
                process.terminate()
                DispatchQueue.global().asyncAfter(deadline: .now() + 2) { if process.isRunning { kill(process.processIdentifier, SIGKILL) } }
            }
            let seconds = Double(limit.components.seconds) + Double(limit.components.attoseconds) / 1e18
            DispatchQueue.global().asyncAfter(deadline: .now() + seconds, execute: watch)
            ended.notify(queue: .global()) {
                watch.cancel()
                reading.readabilityHandler = nil
                // What it said last, without waiting for anything it left running to say more.
                let descriptor = reading.fileDescriptor
                _ = fcntl(descriptor, F_SETFL, fcntl(descriptor, F_GETFL) | O_NONBLOCK)
                var rest = Data()
                var buffer = [UInt8](repeating: 0, count: 4096)
                while true {
                    let count = read(descriptor, &buffer, buffer.count)
                    guard count > 0 else { break }
                    rest.append(contentsOf: buffer[0..<count])
                }
                let last = rest
                let all = said.withLock { state -> Said in
                    state.errors.append(last)
                    return state
                }
                continuation.resume(returning: Result(status: process.terminationStatus, errorText: String(decoding: all.errors.prefix(4096), as: UTF8.self), timedOut: all.timedOut))
            }
        }
    }
}
#endif
