import CryptoKit
import Foundation
import Testing
@testable import KuvoLink

/// A relay in memory: passes `send`s to whoever is connected at the address.
final class MemoryRelay: @unchecked Sendable {
    private let lock = NSLock()
    private var connected: [LinkAddress: Connection] = [:]
    private var registered: [PushRegistration] = []
    private var phones: [LinkAddress: PushRegistration] = [:]
    private var pushed: [(to: LinkAddress, from: LinkAddress, payload: Data)] = []

    /// Each push registration a phone sent, in order.
    var registrations: [PushRegistration] { lock.withLock { registered } }
    /// Each push that went to a phone, as Apple would have it.
    var pushes: [(to: LinkAddress, from: LinkAddress, payload: Data)] { lock.withLock { pushed } }

    final class Connection: RelayTransport, @unchecked Sendable {
        let address: LinkAddress
        let messages: AsyncStream<RelayMessage>
        let inbox: AsyncStream<RelayMessage>.Continuation
        weak var relay: MemoryRelay?

        init(address: LinkAddress, relay: MemoryRelay) {
            self.address = address
            self.relay = relay
            (messages, inbox) = AsyncStream.makeStream()
        }

        func send(_ message: RelayMessage) { relay?.route(message, from: self) }
        func close() {
            inbox.finish()
            relay?.drop(self)
        }
    }

    func connect(_ secret: RelaySecret) -> Connection {
        let connection = Connection(address: secret.address, relay: self)
        lock.withLock {
            connected[secret.address]?.inbox.finish()
            connected[secret.address] = connection
        }
        return connection
    }

    var connect: LinkClient.Connect {
        { _, secret in self.connect(secret) }
    }

    fileprivate func route(_ message: RelayMessage, from sender: Connection) {
        if case .register(let registration) = message {
            lock.withLock {
                registered.append(registration)
                phones[sender.address] = registration
            }
        }
        if case .push(let to, let payload) = message {
            // As the relay answers: sent, or why not.
            let status: RelayMessage.PushStatus = lock.withLock {
                guard let registration = phones[to] else { return .noPhone }
                guard registration.senders.contains(sender.address.text) else { return .notAllowed }
                pushed.append((to, sender.address, payload))
                return .sent
            }
            sender.inbox.yield(.pushed(to: to, status: status))
        }
        guard case .send(let to, let payload) = message else { return }
        let target = lock.withLock { connected[to] }
        if let target {
            target.inbox.yield(.deliver(from: sender.address, payload: payload))
        } else {
            sender.inbox.yield(.unreachable(to))
        }
    }

    fileprivate func drop(_ connection: Connection) {
        lock.withLock { if connected[connection.address] === connection { connected[connection.address] = nil } }
    }
}

/// A host on a memory relay, answering with `handle`, letting in phones that show `token` once.
private struct TestHost {
    let key = LinkKey()
    let secret = RelaySecret()
    let token = randomBytes(16)
    let paired = Paired()
    let server: LinkServer
    let link: PairingLink

    init(relay: MemoryRelay, handle: @escaping LinkServer.Handle, connect: LinkServer.Connect? = nil) {
        server = Self.server(key: key, token: token, paired: paired, handle: handle, connect: connect)
        link = PairingLink(relay: URL(string: "wss://relay.test")!, host: secret.address, key: key.publicBytes, token: token, name: "Test Mac")
        let connection = relay.connect(secret)
        Task { [server] in await server.serve(connection) }
    }

    /// The host started again, as a Mac that quit and opened: the same key and address, the same
    /// phones paired, none of its sessions.
    @discardableResult
    func startedAgain(on relay: MemoryRelay, handle: @escaping LinkServer.Handle) -> LinkServer {
        let server = Self.server(key: key, token: token, paired: paired, handle: handle, connect: nil)
        let connection = relay.connect(secret)
        Task { await server.serve(connection) }
        return server
    }

    private static func server(key: LinkKey, token: Data, paired: Paired, handle: @escaping LinkServer.Handle, connect: LinkServer.Connect?) -> LinkServer {
        LinkServer(key: key, hello: HostHello(kind: .mac, name: "Test Mac", app: "0.0.1", engine: "2.0.30"), authorize: { key, _, hello in
            if paired.contains(key) { return true }
            guard hello.pair == token.base64URL else { return false }
            paired.insert(key)
            return true
        }, handle: handle, connect: connect)
    }

    final class Paired: @unchecked Sendable {
        private let lock = NSLock()
        private var keys: Set<Data> = []
        func contains(_ key: Data) -> Bool { lock.withLock { keys.contains(key) } }
        func insert(_ key: Data) { _ = lock.withLock { keys.insert(key) } }
    }
}

/// A phone's relay connection with a slow way out: each frame takes `each` to leave, in order,
/// and `arriving` more to reach the relay.
private final class SlowConnection: RelayTransport, @unchecked Sendable {
    let base: MemoryRelay.Connection
    private let lock = NSLock()
    private var queued = 0
    private let outbox: AsyncStream<RelayMessage>.Continuation

    init(_ base: MemoryRelay.Connection, each delay: Duration, arriving latency: Duration = .zero) {
        self.base = base
        let (outgoing, outbox) = AsyncStream.makeStream(of: RelayMessage.self)
        let (leaving, left) = AsyncStream.makeStream(of: (ContinuousClock.Instant, RelayMessage).self)
        self.outbox = outbox
        Task { [weak self] in
            for await message in outgoing {
                try? await Task.sleep(for: delay)
                left.yield((.now + latency, message))
                self?.lock.withLock { self?.queued -= 1 }
            }
            left.finish()
        }
        // On the way, in order.
        Task {
            for await (arrives, message) in leaving {
                try? await Task.sleep(until: arrives)
                base.send(message)
            }
        }
    }

    var address: LinkAddress { base.address }
    var messages: AsyncStream<RelayMessage> { base.messages }
    var isSending: Bool { lock.withLock { queued > 0 } }

    func send(_ message: RelayMessage) {
        lock.withLock { queued += 1 }
        outbox.yield(message)
    }

    func close() {
        outbox.finish()
        base.close()
    }
}

/// How many times something happened, from any thread.
private final class Count: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    func add() { lock.withLock { count += 1 } }
    var value: Int { lock.withLock { count } }
}

/// Phones' addresses on the relay, by static key, as a host learns them.
private final class Addresses: @unchecked Sendable {
    private let lock = NSLock()
    private var known: [Data: LinkAddress] = [:]
    func insert(_ key: Data, _ address: LinkAddress) { lock.withLock { known[key] = address } }
    func of(_ key: Data) -> LinkAddress? { lock.withLock { known[key] } }
}

private func phone(on relay: MemoryRelay) -> LinkClient {
    LinkClient(relay: URL(string: "wss://relay.test")!, key: LinkKey(), secret: RelaySecret(), name: "Test iPhone", app: "0.0.1", connect: relay.connect)
}

/// Answers with the request's method, path, headers and body, echoed.
private let echo: LinkServer.Handle = { request, _ in
    AsyncThrowingStream { continuation in
        continuation.yield(.head(status: 200, headers: ["content-type": "application/octet-stream", "x-path": request.path, "x-method": request.method, "x-dir": request.headers["x-opencode-directory"] ?? ""]))
        if !request.body.isEmpty { continuation.yield(.body(request.body)) }
        continuation.finish()
    }
}

private func collect(_ stream: LinkResponse) async throws -> (status: Int, headers: [String: String], body: Data) {
    var status = 0, headers: [String: String] = [:], body = Data()
    for try await part in stream {
        switch part {
        case .head(let s, let h): (status, headers) = (s, h)
        case .body(let data): body += data
        }
    }
    return (status, headers, body)
}

@Suite struct LinkTests {
    @Test func pairsThenAnswers() async throws {
        let relay = MemoryRelay()
        let host = TestHost(relay: relay, handle: echo)
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        #expect(paired.name == "Test Mac")
        #expect(paired.kind == .mac)

        let response = try await collect(client.send(LinkRequest(path: "api/session?limit=5", headers: ["x-opencode-directory": "/tmp/p"]), to: paired))
        #expect(response.status == 200)
        #expect(response.headers["x-path"] == "api/session?limit=5")
        #expect(response.headers["x-dir"] == "/tmp/p")

        // A second request on the same session.
        let posted = try await collect(client.send(LinkRequest(method: "POST", path: "api/x", body: Data("hello".utf8)), to: paired))
        #expect(posted.headers["x-method"] == "POST")
        #expect(posted.body == Data("hello".utf8))
    }

    @Test func aPhoneThatIsNotPairedIsTurnedAway() async throws {
        let relay = MemoryRelay()
        let host = TestHost(relay: relay, handle: echo)
        let stranger = phone(on: relay)
        var link = host.link
        link.token = randomBytes(16)
        await #expect(throws: LinkError.refused) { try await stranger.pair(link) }

        let paired = PairedHost(address: host.secret.address, key: host.key.publicBytes, name: "", kind: .mac)
        await #expect(throws: LinkError.refused) { try await collect(stranger.send(LinkRequest(path: "api/session"), to: paired)) }
    }

    @Test func aPairedPhoneComesBackWithoutTheToken() async throws {
        let relay = MemoryRelay()
        let host = TestHost(relay: relay, handle: echo)
        let key = LinkKey(), secret = RelaySecret()
        let first = LinkClient(relay: host.link.relay, key: key, secret: secret, name: "p", app: "0", connect: relay.connect)
        let paired = try await first.pair(host.link)
        await first.disconnect()

        let later = LinkClient(relay: host.link.relay, key: key, secret: secret, name: "p", app: "0", connect: relay.connect)
        #expect(try await collect(later.send(LinkRequest(path: "api/info"), to: paired)).status == 200)
    }

    @Test func aHostStartedAgainGetsANewSessionAndRunsTheRequestOnce() async throws {
        let relay = MemoryRelay()
        let runs = Count()
        let counting: LinkServer.Handle = { request, phone in
            if request.method == "POST" { runs.add() }
            return echo(request, phone)
        }
        let host = TestHost(relay: relay, handle: counting)
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        #expect(try await collect(client.send(LinkRequest(path: "api/info"), to: paired)).status == 200)

        // The Mac quits and opens again; the phone's session is one it no longer knows.
        host.startedAgain(on: relay, handle: counting)
        await client.setTimingForTesting(LinkClient.Timing(beforeRead: .zero, beforeChange: .zero))
        let posted = try await collect(client.send(LinkRequest(method: "POST", path: "api/x", body: Data("once".utf8)), to: paired))
        #expect(posted.status == 200)
        #expect(posted.body == Data("once".utf8))
        #expect(runs.value == 1)
    }

    @Test func aQuietSessionToAHostThatLeftSaysSoWithoutWaiting() async throws {
        let relay = MemoryRelay()
        let host = TestHost(relay: relay, handle: echo)
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        await client.setTimingForTesting(LinkClient.Timing(beforeRead: .zero, beforeChange: .zero))
        // Its relay connection gone (the Mac asleep): the check hears it's unreachable at once.
        relay.connect(host.secret).close()
        let began = ContinuousClock.now
        await #expect(throws: LinkError.hostOffline) { try await collect(client.send(LinkRequest(path: "api/info"), to: paired)) }
        #expect(ContinuousClock.now - began < .seconds(3))
    }

    @Test(.timeLimit(.minutes(1))) func aRequestLostOnTheWayFailsRatherThanWaitingForever() async throws {
        let relay = MemoryRelay()
        let host = TestHost(relay: relay, handle: echo)
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        await client.setTimingForTesting(LinkClient.Timing(answer: .milliseconds(100), check: .milliseconds(200)))
        // Something still connected at the host's address that takes frames and never answers,
        // as a host's old relay connection does while it starts again.
        let deaf = relay.connect(host.secret)
        let began = ContinuousClock.now
        await #expect(throws: LinkError.timedOut) { try await collect(client.send(LinkRequest(path: "api/info"), to: paired)) }
        #expect(ContinuousClock.now - began < .seconds(3))
        _ = deaf
    }

    @Test func aBigBodyOnASlowConnectionIsntCutOff() async throws {
        let relay = MemoryRelay()
        let host = TestHost(relay: relay, handle: echo)
        let client = LinkClient(relay: URL(string: "wss://relay.test")!, key: LinkKey(), secret: RelaySecret(), name: "Test iPhone", app: "0.0.1") { _, secret in
            SlowConnection(relay.connect(secret), each: .milliseconds(20))
        }
        let paired = try await client.pair(host.link)
        await client.setTimingForTesting(LinkClient.Timing(beforeChange: .zero, answer: .milliseconds(100), check: .milliseconds(200)))
        // About twenty pieces, each taking 20 ms to leave: longer than a check may take.
        let big = randomBytes(1_300_000)
        let response = try await collect(client.send(LinkRequest(method: "POST", path: "api/upload", body: big), to: paired))
        #expect(response.body == big)
    }

    @Test func aRequestAlongsideABigBodyOnASlowConnectionIsntCutOff() async throws {
        // A second request's check goes out behind the body; it starting at different points
        // tells whether the check counts from when its ping leaves.
        for offset in [0, 100, 200, 250, 300, 350, 400] {
            let relay = MemoryRelay()
            let host = TestHost(relay: relay, handle: echo)
            let client = LinkClient(relay: URL(string: "wss://relay.test")!, key: LinkKey(), secret: RelaySecret(), name: "Test iPhone", app: "0.0.1") { _, secret in
                SlowConnection(relay.connect(secret), each: .milliseconds(20), arriving: .milliseconds(150))
            }
            let paired = try await client.pair(host.link)
            await client.setTimingForTesting(LinkClient.Timing(beforeChange: .zero, answer: .milliseconds(100), check: .milliseconds(300)))
            let big = randomBytes(1_300_000)
            async let upload = collect(client.send(LinkRequest(method: "POST", path: "api/upload", body: big), to: paired))
            try await Task.sleep(for: .milliseconds(offset))
            let small = try await collect(client.send(LinkRequest(method: "POST", path: "api/small", body: Data("x".utf8)), to: paired))
            #expect(small.status == 200, "starting at \(offset) ms")
            #expect(try await upload.body == big, "starting at \(offset) ms")
        }
    }

    @Test func aSlowAnswerFromAHostThatIsThereIsWaitedFor() async throws {
        let relay = MemoryRelay()
        let slow: LinkServer.Handle = { request, _ in
            AsyncThrowingStream { continuation in
                Task {
                    try? await Task.sleep(for: .milliseconds(600))
                    continuation.yield(.head(status: 200, headers: [:]))
                    continuation.finish()
                }
            }
        }
        let host = TestHost(relay: relay, handle: slow)
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        await client.setTimingForTesting(LinkClient.Timing(answer: .milliseconds(100), check: .milliseconds(200)))
        #expect(try await collect(client.send(LinkRequest(path: "api/slow"), to: paired)).status == 200)
    }

    @Test func bodiesLongerThanOneMessageArriveWhole() async throws {
        let relay = MemoryRelay()
        let host = TestHost(relay: relay, handle: echo)
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        let big = randomBytes(300_000)
        let response = try await collect(client.send(LinkRequest(method: "POST", path: "api/upload", body: big), to: paired))
        #expect(response.body == big)
    }

    @Test(.timeLimit(.minutes(1))) func aHostThatStopsTellsItsPhones() async throws {
        let relay = MemoryRelay()
        // An event stream that never ends by itself.
        let host = TestHost(relay: relay) { request, phone in
            guard request.path == "api/event" else { return echo(request, phone) }
            return AsyncThrowingStream { continuation in
                let task = Task {
                    continuation.yield(.head(status: 200, headers: ["content-type": "text/event-stream"]))
                    continuation.yield(.body(Data("data: hello\n\n".utf8)))
                    while !Task.isCancelled { try? await Task.sleep(for: .milliseconds(50)) }
                }
                continuation.onTermination = { _ in task.cancel() }
            }
        }
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        await client.setTimingForTesting(LinkClient.Timing(answer: .seconds(30), check: .seconds(30)))
        var parts = client.send(LinkRequest(path: "api/event"), to: paired).makeAsyncIterator()
        // Its head, and its first event.
        _ = try await parts.next()
        _ = try await parts.next()
        // The host stops (its engine starting over): the phone hears at once, not after its checks.
        let began = ContinuousClock.now
        await host.server.closeAll()
        await #expect(throws: LinkError.closed) { _ = try await parts.next() }
        #expect(ContinuousClock.now - began < .seconds(2))
        // What it asks next goes on a new session.
        #expect(try await collect(client.send(LinkRequest(path: "api/info"), to: paired)).status == 200)
    }

    @Test func aStreamStopsOnTheHostWhenThePhoneStopsReading() async throws {
        let relay = MemoryRelay()
        let stopped = AsyncStream.makeStream(of: Void.self)
        let host = TestHost(relay: relay) { request, phone in
            guard request.path == "api/event" else { return echo(request, phone) }
            return AsyncThrowingStream { continuation in
                let task = Task {
                    continuation.yield(.head(status: 200, headers: ["content-type": "text/event-stream"]))
                    var n = 0
                    while !Task.isCancelled {
                        continuation.yield(.body(Data("data: \(n)\n\n".utf8)))
                        n += 1
                        try? await Task.sleep(for: .milliseconds(5))
                    }
                    stopped.continuation.yield()
                }
                continuation.onTermination = { _ in task.cancel() }
            }
        }
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        var events = 0
        for try await part in client.send(LinkRequest(path: "api/event"), to: paired) {
            if case .body = part { events += 1 }
            if events == 3 { break }
        }
        #expect(events == 3)
        var iterator = stopped.stream.makeAsyncIterator()
        _ = await iterator.next()

        // The session is still good for the next request.
        #expect(try await collect(client.send(LinkRequest(path: "api/info"), to: paired)).status == 200)
    }

    @Test func aStreamStopsOnTheHostWhenThePhoneIsGone() async throws {
        let relay = MemoryRelay()
        let stopped = AsyncStream.makeStream(of: Void.self)
        let host = TestHost(relay: relay) { request, phone in
            guard request.path == "api/event" else { return echo(request, phone) }
            return AsyncThrowingStream { continuation in
                let task = Task {
                    continuation.yield(.head(status: 200, headers: ["content-type": "text/event-stream"]))
                    while !Task.isCancelled {
                        continuation.yield(.body(Data("data: {}\n\n".utf8)))
                        try? await Task.sleep(for: .milliseconds(5))
                    }
                    stopped.continuation.yield()
                }
                continuation.onTermination = { _ in task.cancel() }
            }
        }
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        // The phone goes away mid-stream without a word, as one put to sleep does.
        let reading = Task { for try await _ in client.send(LinkRequest(path: "api/event"), to: paired) {} }
        try await Task.sleep(for: .milliseconds(50))
        await client.dropRelayForTesting()
        reading.cancel()
        var iterator = stopped.stream.makeAsyncIterator()
        _ = await iterator.next()
    }

    @Test func aPushRegistrationGoesAgainEachTimeThePhoneConnects() async throws {
        let relay = MemoryRelay()
        let host = TestHost(relay: relay, handle: echo)
        let client = phone(on: relay)
        let registration = PushRegistration(token: "a1b2", environment: "sandbox", topic: "run.kuvo.code.mobile", senders: [host.secret.address])
        // Sent at once, connecting for it.
        await client.register(registration)
        #expect(relay.registrations == [registration])
        // Woken up after the relay went: sent again with the new connection.
        let paired = try await client.pair(host.link)
        await client.dropRelayForTesting()
        try await Task.sleep(for: .milliseconds(50))
        _ = try await client.hostHello(paired)
        #expect(relay.registrations == [registration, registration])
        // Stopped: the next connection goes without it.
        await client.register(nil)
        await client.dropRelayForTesting()
        try await Task.sleep(for: .milliseconds(50))
        _ = try await client.hostHello(paired)
        #expect(relay.registrations.count == 2)
    }

    @Test func aHostPushesToThePhonesItKnows() async throws {
        let relay = MemoryRelay()
        let statuses = AsyncStream.makeStream(of: RelayMessage.PushStatus.self)
        let addresses = Addresses()
        let key = LinkKey(), secret = RelaySecret(), token = randomBytes(16)
        let server = LinkServer(key: key, hello: HostHello(kind: .mac, name: "Test Mac", app: "0.0.1", engine: "2.0.30"), authorize: { phone, address, _ in
            addresses.insert(phone, address)
            return true
        }, handle: echo, pushed: { _, status in statuses.continuation.yield(status) })
        let connection = relay.connect(secret)
        Task { await server.serve(connection) }
        let phoneKey = LinkKey(), phoneSecret = RelaySecret()
        let client = LinkClient(relay: URL(string: "wss://relay.test")!, key: phoneKey, secret: phoneSecret, name: "Test iPhone", app: "0.0.1", connect: relay.connect)
        _ = try await client.pair(PairingLink(relay: URL(string: "wss://relay.test")!, host: secret.address, key: key.publicBytes, token: token, name: "Test Mac"))
        // Learned when the phone came in: where its pushes go.
        let address = try #require(addresses.of(phoneKey.publicBytes))
        #expect(address == phoneSecret.address)
        #expect(await server.hasSessions)
        let notice = Notice(title: "Fixer", body: "Done: the tests pass.", category: .report, thread: "ses_1", data: ["session": "ses_1"])
        var heard = statuses.stream.makeAsyncIterator()

        // Not registered yet: the relay says so.
        #expect(await server.push(try notice.encoded(), to: [(phoneKey.publicBytes, address)]))
        #expect(await heard.next() == .noPhone)

        await client.register(PushRegistration(token: "a1b2", environment: "sandbox", topic: "run.kuvo.code.mobile", senders: [secret.address]))
        try await Task.sleep(for: .milliseconds(20))
        await server.push(try notice.encoded(), to: [(phoneKey.publicBytes, address)])
        #expect(await heard.next() == .sent)
        // Only the phone opens it, as from this host.
        let push = try #require(relay.pushes.first)
        #expect(push.from == secret.address)
        let opened = try Mail.open(push.payload, to: phoneKey, from: key.publicBytes)
        #expect(try JSONDecoder().decode(Notice.self, from: opened) == notice)
        #expect(throws: (any Error).self) { try Mail.open(push.payload, to: LinkKey(), from: key.publicBytes) }
    }

    @Test func aCallbackCalledAgainIsLetGo() async {
        // As URLSession's pong handler can be: an answer, then an error as the task closes, the
        // second from another queue, a moment later.
        let answered = await firstAnswer { (answer: @escaping @Sendable (Bool) -> Void) in
            answer(true)
            answer(false)
            DispatchQueue.global().asyncAfter(deadline: .now() + .milliseconds(20)) { answer(false) }
        }
        #expect(answered)
        let later = await firstAnswer { (answer: @escaping @Sendable (Int) -> Void) in
            DispatchQueue.global().async { answer(1) }
            DispatchQueue.global().async { answer(2) }
        }
        #expect(later == 1 || later == 2)
        try? await Task.sleep(for: .milliseconds(50))
    }

    @Test func aLongNoticeIsCutToFit() throws {
        let body = String(repeating: "Wörds and more words. ", count: 400)
        let notice = Notice(title: "Fixer", subtitle: "kuvo-ai", body: body, category: .finished, thread: "ses_1", data: ["session": "ses_1"])
        let data = try notice.encoded()
        #expect(data.count <= Notice.limit)
        let cut = try JSONDecoder().decode(Notice.self, from: data)
        #expect(cut.body.hasSuffix("…"))
        #expect(body.hasPrefix(String(cut.body.dropLast())))
        #expect(cut.body.count > 2000)
        // Short ones go as they are.
        let short = Notice(title: "Fixer", body: "Done.", category: .finished, thread: "ses_1", data: [:])
        #expect(try JSONDecoder().decode(Notice.self, from: short.encoded()) == short)
        // A category from a newer host still decodes, to be shown plainly.
        let newer = try JSONDecoder().decode(Notice.self, from: Data(#"{"t":"notify","title":"Fixer","body":"Hi","category":"reminder","thread":"ses_1","data":{}}"#.utf8))
        #expect(newer.category.rawValue == "reminder")
        #expect(String(decoding: try short.encoded(), as: UTF8.self).contains(#""category":"finished""#))
    }

    @Test func socketsCarryMessagesBothWaysUntilClosed() async throws {
        let relay = MemoryRelay()
        let closed = AsyncStream.makeStream(of: Void.self)
        let note = Data([0]) + Data(#"{"cursor":0}"#.utf8)
        // A terminal that answers each line it's sent, after a note of where its output starts.
        let host = TestHost(relay: relay, handle: echo) { request, _, input in
            AsyncThrowingStream { continuation in
                guard request.path == "api/pty/pty_1/connect" else { return continuation.finish(throwing: URLError(.badURL)) }
                continuation.yield(.open)
                continuation.yield(.binary(note))
                let task = Task {
                    for await text in input { continuation.yield(.text(Data("echo:".utf8) + text)) }
                    closed.continuation.yield()
                    continuation.finish()
                }
                continuation.onTermination = { _ in task.cancel() }
            }
        }
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        let socket = client.socket(.socket("api/pty/pty_1/connect"), to: paired)
        var events = socket.events.makeAsyncIterator()
        #expect(try await events.next() == .open)
        #expect(try await events.next() == .binary(note))
        try await socket.send(Data("ls\r".utf8))
        #expect(try await events.next() == .text(Data("echo:ls\r".utf8)))
        // Closing it ends it on the host.
        socket.close()
        var stops = closed.stream.makeAsyncIterator()
        _ = await stops.next()

        // One the host won't open fails, and the session carries on.
        let refused = client.socket(.socket("api/pty/pty_2/connect"), to: paired)
        await #expect(throws: LinkError.notAvailable) { for try await _ in refused.events {} }
        #expect(try await collect(client.send(LinkRequest(path: "api/info"), to: paired)).status == 200)
    }

    @Test func aHostWithoutSocketsTurnsThemAway() async throws {
        let relay = MemoryRelay()
        let host = TestHost(relay: relay, handle: echo)
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        await #expect(throws: LinkError.notAvailable) { for try await _ in client.socket(.socket("api/pty/pty_1/connect"), to: paired).events {} }
    }

    @Test func aHostThatIsNotConnectedIsOffline() async throws {
        let relay = MemoryRelay()
        let client = phone(on: relay)
        let nobody = PairedHost(address: RelaySecret().address, key: LinkKey().publicBytes, name: "", kind: .machine)
        await #expect(throws: LinkError.hostOffline) { try await collect(client.send(LinkRequest(path: "api/info"), to: nobody)) }
    }

    @Test func aFailingHandlerAnswers502() async throws {
        let relay = MemoryRelay()
        struct Broken: Error {}
        let host = TestHost(relay: relay) { _, _ in AsyncThrowingStream { $0.finish(throwing: Broken()) } }
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        #expect(try await collect(client.send(LinkRequest(path: "api/info"), to: paired)).status == 502)
    }

    @Test func urlSessionRequestsGoOverTheLink() async throws {
        let relay = MemoryRelay()
        let host = TestHost(relay: relay, handle: echo)
        let client = phone(on: relay)
        let paired = try await client.pair(host.link)
        let session = URLSession(configuration: LinkURLProtocol.configuration(client: client) { [paired] })
        let base = LinkURLProtocol.baseURL(for: paired)

        var request = URLRequest(url: base.appending(path: "api/session/abc/prompt").appending(queryItems: [URLQueryItem(name: "a", value: "b c")]))
        request.httpMethod = "POST"
        request.httpBody = Data(#"{"text":"hi"}"#.utf8)
        request.setValue("/Users/me/project", forHTTPHeaderField: "x-opencode-directory")
        let (data, response) = try await session.data(for: request)
        let http = try #require(response as? HTTPURLResponse)
        #expect(http.statusCode == 200)
        #expect(http.value(forHTTPHeaderField: "x-path") == "api/session/abc/prompt?a=b%20c")
        #expect(http.value(forHTTPHeaderField: "x-dir") == "/Users/me/project")
        #expect(data == Data(#"{"text":"hi"}"#.utf8))

        let (bytes, _) = try await session.bytes(for: URLRequest(url: base.appending(path: "api/event")))
        var lines = 0
        for try await _ in bytes.lines { lines += 1 }
        #expect(lines == 0)
    }

    @Test func mailOpensOnlyForItsPhone() throws {
        let host = LinkKey(), phone = LinkKey(), other = LinkKey()
        let payload = Data(#"{"t":"notify","title":"Needs you"}"#.utf8)
        let frame = try Mail.seal(payload, from: host, to: phone.publicBytes)
        #expect(try Mail.open(frame, to: phone, from: host.publicBytes) == payload)
        #expect(throws: Noise.Failure.decrypt) { try Mail.open(frame, to: other, from: host.publicBytes) }
        #expect(throws: Noise.Failure.decrypt) { try Mail.open(frame, to: phone, from: other.publicBytes) }
    }

    @Test func pairingLinksGoBothWays() throws {
        let link = PairingLink(relay: URL(string: "wss://link.example/v1")!, host: RelaySecret().address, key: LinkKey().publicBytes, name: "Sam’s MacBook & co")
        #expect(PairingLink(link.url) == link)
        #expect(PairingLink(URL(string: "kuvo-link://pair?v=2")!) == nil)
        #expect(PairingLink(URL(string: "https://pair")!) == nil)
    }

    @Test func messagesSplitToFit() throws {
        let body = randomBytes(200_000)
        let pieces = LinkMessage.pieces(.init(t: .res, id: 9, h: ["content-type": "x"], s: 200), body: body, end: true)
        #expect(pieces.count == 4)
        #expect(pieces.allSatisfy { $0.data.count <= LinkMessage.maxLength })
        #expect(pieces.first?.head.t == .res)
        #expect(pieces.dropFirst().allSatisfy { $0.head.t == .data && $0.head.id == 9 })
        #expect(pieces.map(\.head.end) == [false, false, false, true])
        #expect(pieces.reduce(Data()) { $0 + $1.body } == body)
        for piece in pieces { #expect(try LinkMessage(piece.data) == piece) }
    }
}
