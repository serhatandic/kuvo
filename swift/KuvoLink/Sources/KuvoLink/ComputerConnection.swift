/// The route this client chooses for one computer.
public enum ComputerConnectionMethod: String, Codable, Sendable {
    case automatic, ssh, relay
}

/// Saved by this client with the computer's public key. The SSH alias belongs to this client.
public struct ComputerConnectionPreference: Codable, Equatable, Sendable {
    public var method: ComputerConnectionMethod
    public var destination: String?

    public init(method: ComputerConnectionMethod = .automatic, destination: String? = nil) {
        self.method = method
        self.destination = destination
    }
}

public enum ComputerConnectionError: Error, Equatable, Sendable {
    case sshUnavailable
}

/// Used by clients with an SSH transport. Each probe and request must verify the box's public key.
public struct ComputerSSH<Reply: Sendable>: Sendable {
    let available: @Sendable (PairedHost, String?) async -> Bool
    let send: @Sendable (LinkRequest, PairedHost, String?) async throws -> Reply

    public init(
        available: @escaping @Sendable (PairedHost, String?) async -> Bool,
        send: @escaping @Sendable (LinkRequest, PairedHost, String?) async throws -> Reply
    ) {
        self.available = available
        self.send = send
    }
}

/// Routes an unchanged request using the device's choice. Clients without SSH use relay in Auto.
public struct ComputerConnection<Reply: Sendable>: Sendable {
    private let relay: @Sendable (LinkRequest, PairedHost) async throws -> Reply
    private let ssh: ComputerSSH<Reply>?

    public init(
        relay: @escaping @Sendable (LinkRequest, PairedHost) async throws -> Reply,
        ssh: ComputerSSH<Reply>? = nil
    ) {
        self.relay = relay
        self.ssh = ssh
    }

    public func send(_ request: LinkRequest, to host: PairedHost, preference: ComputerConnectionPreference = .init()) async throws -> Reply {
        if preference.method != .relay, let ssh, await ssh.available(host, preference.destination) {
            try Task.checkCancellation()
            // A dropped connection might already have applied this request.
            return try await ssh.send(request, host, preference.destination)
        }
        try Task.checkCancellation()
        if preference.method == .ssh { throw ComputerConnectionError.sshUnavailable }
        return try await relay(request, host)
    }
}

extension LinkClient {
    public nonisolated func connection(ssh: ComputerSSH<LinkResponse>? = nil) -> ComputerConnection<LinkResponse> {
        ComputerConnection(relay: { request, host in self.send(request, to: host) }, ssh: ssh)
    }
}
