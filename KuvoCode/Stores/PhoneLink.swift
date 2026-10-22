import AppKit
import CryptoKit
import Foundation
import KuvoLink
import OpenCodeKit
import os
import UsageKit
import Security
import SystemConfiguration

/// Kuvo Code for iPhone's way in: this Mac answers the phones you've paired, through the relay,
/// passing what they ask to its engine. Everything between them is encrypted end to end
/// (`Packages/KuvoLink`); the relay only passes bytes along.
///
/// Off until the phone app is out: `defaults write run.kuvo.code phoneLink -bool true`, or
/// KUVO_DEMO_PHONE=1, turns it on, with Settings › Phone.
@Observable
final class PhoneLink {
    static var isEnabled: Bool {
        UserDefaults.standard.bool(forKey: "phoneLink") || ProcessInfo.processInfo.environment["KUVO_DEMO_PHONE"] != nil
    }

    /// Where the relay is until there's a public one. `defaults write run.kuvo.code
    /// phoneLink.relay wss://…` points elsewhere.
    static let defaultRelay = URL(string: "ws://127.0.0.1:8790")!
    static let pairingTime: Duration = .seconds(600)

    struct Phone: Codable, Hashable, Identifiable {
        /// Its static key, base64url.
        var key: String
        var name: String
        var added: Date
        /// Its address on the relay, where its notifications go: learned as it comes in.
        var address: String?
        var id: String { key }
    }

    enum Status: Equatable {
        case off, connecting, connected
        case failed(String)
    }

    private(set) var phones: [Phone] = []
    /// Your machines' bridges, which phones reach directly (with the Mac closed), by machine:
    /// what `kuvo/hosts` tells a phone.
    private(set) var machineHosts: [String: MachineHost] = [:]
    /// The machines that are connected now, for putting the bridge on them; set by AppModel.
    @ObservationIgnored var readyMachines: () -> [MachineEngine] = { [] }
    /// Your bots, for `kuvo/bots`: the phone draws and describes each as the Mac does; set by
    /// AppModel.
    @ObservationIgnored weak var bots: Bots?
    /// Every conversation's wake-ups and watches, for `kuvo/wakes`; set by AppModel.
    @ObservationIgnored weak var wakeUps: WakeUps?
    /// What agents ask you for while they browse, for `kuvo/asks`: a phone answers them as the
    /// cards in the window do; set by AppModel.
    @ObservationIgnored weak var browserAsks: BrowserAsks?
    /// Your group chats, for `kuvo/groups`: a phone reads and writes in them as the window does;
    /// set by AppModel.
    @ObservationIgnored weak var groups: GroupChats?
    /// Your sessions, for stopping a command an agent left running (`kuvo/shell/<id>/stop`); set
    /// by AppModel.
    @ObservationIgnored weak var workspace: Workspace?
    /// A machine's name, by its id, for where an ask's browser runs; set by AppModel.
    @ObservationIgnored var machineName: (String) -> String? = { _ in nil }
    /// The agents bridge (Claude Code, Codex…), for `kuvo/agents/api/…`; set by AppModel.
    @ObservationIgnored var agents: OutsideAgents?
    /// Plans whose sign-in ended (the service refused it), for `kuvo/accounts/ended`, and
    /// Disconnect as Settings › Accounts does it (here and on your machines); set by AppModel.
    @ObservationIgnored var endedSignIns: () -> [String] = { [] }
    /// (Whether it was one of your plans: nothing else is signed out.)
    @ObservationIgnored var forgetSignIn: (String) -> Bool = { _ in false }
    /// Whether a group chat is on its way to another computer; set by AppModel.
    @ObservationIgnored var isMoving: (String) -> Bool = { Moves.isFrozen($0) }
    /// This Mac's engine settings, for `kuvo/config` and `kuvo/mcp/catalog`; set by AppModel.
    @ObservationIgnored weak var engineConfig: EngineConfig?
    /// Starts this Mac's engine over, as Settings › Engine's Restart does; set by AppModel.
    @ObservationIgnored var restartEngine: () -> Void = {}
    /// Whether a machine's engine is another Mac's, whose settings come from that Mac; set by AppModel.
    @ObservationIgnored var isAnotherMacs: (String) -> Bool = { _ in false }

    struct MachineHost: Codable, Hashable {
        var name: String
        var address: String
        var key: String
    }

    /// What's kept in phone-link.json.
    private struct Saved: Codable {
        var phones: [Phone] = []
        var machines: [String: MachineHost] = [:]
    }
    private(set) var status = Status.off
    /// The link a phone pairs with, while Settings › Phone shows it: good for ten minutes, once.
    private(set) var pairing: PairingLink?
    @ObservationIgnored private var pairingEnds = ContinuousClock.now

    var relay: URL {
        get { UserDefaults.standard.string(forKey: "phoneLink.relay").flatMap(URL.init(string:)) ?? Self.defaultRelay }
        set {
            UserDefaults.standard.set(newValue.absoluteString, forKey: "phoneLink.relay")
            restart()
            syncMachines()
        }
    }

    /// This Mac's key and relay secret, from the Keychain, made the first time the link is on
    /// (with it off, Kuvo Code keeps nothing for it).
    @ObservationIgnored private lazy var identity: (key: LinkKey, secret: RelaySecret) = Self.loadIdentity()
    private var key: LinkKey { identity.key }
    private var secret: RelaySecret { identity.secret }
    @ObservationIgnored private var server: LinkServer?
    @ObservationIgnored private var running: Task<Void, Never>?
    @ObservationIgnored private var presence: Task<Void, Never>?
    /// When this Mac last told each machine's bridge it's here, by machine.
    @ObservationIgnored private var markedPresent: [String: ContinuousClock.Instant] = [:]
    /// How long that lets this Mac tell a machine's news: less than the bridge waits before it
    /// tells it (`PRESENT_MS` in kuvo-link.js), so the two never both do, after a sleep say.
    private static let presentFor: Duration = .seconds(120)
    @ObservationIgnored private var endpoint: ServerEndpoint?
    private let log = Logger(subsystem: "run.kuvo.code", category: "phone")

    private static var file: URL { AppInfo.supportDirectory.appending(path: "phone-link.json") }

    init() {
        let saved = (try? JSONDecoder().decode(Saved.self, from: Data(contentsOf: Self.file))) ?? Saved()
        phones = saved.phones
        machineHosts = saved.machines
    }

    private static func loadIdentity() -> (key: LinkKey, secret: RelaySecret) {
        (kept("key", read: { try? LinkKey(rawRepresentation: $0) }, make: LinkKey.init, bytes: \.rawRepresentation),
         kept("secret", read: RelaySecret.init(bytes:), make: RelaySecret.init, bytes: \.bytes))
    }

    /// One of them from the Keychain, made the first time. One the Keychain won't give out (you
    /// said no when it asked) isn't replaced: phones won't reach this Mac until it's opened again.
    private static func kept<T>(_ account: String, read: (Data) -> T?, make: () -> T, bytes: (T) -> Data) -> T {
        do {
            if let data = try keychain(account), let value = read(data) { return value }
        } catch {
            Logger(subsystem: "run.kuvo.code", category: "phone").error("Couldn’t read the phone link’s \(account, privacy: .public) from the Keychain: \(error.localizedDescription, privacy: .public)")
            return make()
        }
        let value = make()
        setKeychain(bytes(value), for: account)
        return value
    }

    /// This Mac's name, as Sharing settings has it.
    static var computerName: String {
        (SCDynamicStoreCopyComputerName(nil, nil) as String?) ?? "Mac"
    }

    /// Starts answering phones, through this Mac's engine at `endpoint`, once it's ready.
    func start(endpoint: ServerEndpoint) {
        self.endpoint = endpoint
        guard Self.isEnabled, running == nil else { return }
        server = makeServer(endpoint: endpoint)
        running = Task { await run() }
        keepPresent()
    }

    func stop() {
        running?.cancel()
        running = nil
        presence?.cancel()
        presence = nil
        server = nil
        status = .off
    }

    private func restart() {
        guard let endpoint else { return }
        stop()
        start(endpoint: endpoint)
    }

    /// Shows a pairing link (as a QR code) for a phone to scan.
    func beginPairing() {
        pairing = PairingLink(relay: relay, host: secret.address, key: key.publicBytes, name: Self.computerName)
        pairingEnds = .now + Self.pairingTime
    }

    func endPairing() {
        pairing = nil
    }

    func remove(_ phone: Phone) {
        phones.removeAll { $0.id == phone.id }
        save()
        if let key = Data(base64URL: phone.key) { Task { [server] in await server?.forget(phone: key) } }
        syncMachines()
    }

    // MARK: Machines

    /// Puts the bridge on each connected machine, with the phones you've paired now.
    func syncMachines() {
        for engine in readyMachines() { Task { await sync(engine) } }
    }

    /// Puts the bridge (`kuvo-link.js`) on `engine`'s machine, in `~/.kuvo-code/bin`, with the
    /// phones you've paired and the relay, so they reach it directly, with this Mac closed too.
    /// The machine keeps its own keys and tells this Mac only its address and public key. With
    /// no phones, the bridge stops there.
    func sync(_ engine: MachineEngine) async {
        guard Self.isEnabled, engine.isReady else { return }
        let machine = engine.machine
        let folder = FileManager.default.temporaryDirectory.appending(path: Bundle.main.bundleIdentifier ?? "run.kuvo.code").appending(path: "phone-link-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: folder) }
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let script = try Bridge.script
            let hash = SHA256.hash(data: script).map { String(format: "%02x", $0) }.joined()
            let scriptFile = folder.appending(path: Bridge.fileName)
            try script.write(to: scriptFile)
            try await machine.connection.run(RemoteEngineInstaller.script("""
                f="$K/bin/\(Bridge.fileName)"
                if [ "$(sha256sum "$f" 2>/dev/null | cut -c1-64)" = "\(hash)" ]; then cat >/dev/null; else cat >"$f.new" && mv -f "$f.new" "$f"; fi
                """), input: scriptFile)

            struct Setup: Encodable {
                struct Phone: Encodable { var key: String; var name: String; var address: String? }
                var relay: String
                var name: String
                var phones: [Phone]
            }
            let settings = folder.appending(path: "setup.json")
            try JSONEncoder().encode(Setup(relay: relay.absoluteString, name: machine.name, phones: phones.map { .init(key: $0.key, name: $0.name, address: $0.address) })).write(to: settings)
            let output = try await machine.connection.run(RemoteEngineInstaller.script("""
                KUVO_HOME="$K" BUN_BE_BUN=1 BUN_RUNTIME_TRANSPILER_CACHE_PATH=0 "$E" "$K/bin/\(Bridge.fileName)" setup
                """), input: settings)
            struct Answer: Decodable { var address: String?; var key: String? }
            let answer = try? JSONDecoder().decode(Answer.self, from: Data(output.utf8))
            if let address = answer?.address, let key = answer?.key {
                machineHosts[machine.id] = MachineHost(name: machine.name, address: address, key: key)
            } else {
                machineHosts[machine.id] = nil
            }
            save()
            // Here from the start, not from the next minute: the bridge leaves this machine's news to this Mac.
            if status == .connected, phones.contains(where: { $0.address != nil }) { await markPresent(on: machine) }
        } catch {
            log.error("Couldn't set up the phone link on a machine: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// Tells each connected machine's bridge, every minute, that this Mac is here and tells your
    /// phones what happens in the machine's sessions itself: the bridge does it only while no
    /// Mac is (`RemoteServer.markPresent`), so nothing comes twice.
    private func keepPresent() {
        presence?.cancel()
        presence = Task { [weak self] in
            while !Task.isCancelled {
                if let self, status == .connected, phones.contains(where: { $0.address != nil }) {
                    for engine in readyMachines() where engine.isLive { await markPresent(on: engine.machine) }
                }
                try? await Task.sleep(for: .seconds(60))
            }
        }
    }

    /// The machines whose bridge may still think this Mac is here (`PRESENT_MS` in kuvo-link.js),
    /// for telling them it's gone as it quits.
    var presentOn: [String] {
        markedPresent.filter { .now - $0.value < .seconds(150) }.map(\.key)
    }

    private func markPresent(on machine: Machine) async {
        let at = ContinuousClock.now
        if await RemoteServer.markPresent(MachineEngine.macID, on: machine.connection) { markedPresent[machine.id] = at }
    }

    /// A machine you removed: phones stop being told about it.
    func forget(machine id: String) {
        guard machineHosts.removeValue(forKey: id) != nil else { return }
        save()
    }

    // MARK: Answering

    private func run() async {
        var attempt = 0
        while !Task.isCancelled, let server {
            status = .connecting
            do {
                let connection = try await WebSocketRelay.connect(to: relay, secret: secret)
                status = .connected
                attempt = 0
                await server.serve(connection)
            } catch {
                status = .failed("Can’t reach the relay at \(relay.host() ?? relay.absoluteString).")
            }
            attempt += 1
            try? await Task.sleep(for: .seconds(min(60, 1 << min(attempt, 6))))
        }
    }

    private func makeServer(endpoint: ServerEndpoint) -> LinkServer {
        let hello = HostHello(kind: .mac, name: Self.computerName, app: AppInfo.version, engine: OpenCodeEngine.version)
        let base = endpoint.url.absoluteString.hasSuffix("/") ? endpoint.url : URL(string: endpoint.url.absoluteString + "/")!
        let password = endpoint.password
        let log = log
        let agents = agents?.address
        return LinkServer(key: key, hello: hello) { [weak self] key, address, hello in
            await self?.authorize(key, address, hello) ?? false
        } handle: { [weak self] request, _ in
            Self.respond(to: request, link: self, engine: base, password: password, agents: agents)
        } connect: { request, _, input in
            // Terminals: the engine's, in a project's folder.
            EngineRoutes.connect(request, to: base, password: password, input: input)
        } pushed: { _, status in
            switch status {
            case .sent: break
            // Until the phone has registered for notifications, and asked for this Mac's.
            case .noPhone: log.info("A push had no phone registered for it")
            case .notAllowed: log.info("A push wasn't from one of the phone's hosts")
            case .refused: log.error("Apple refused a push")
            }
        } confirmed: { [weak self] phone in
            Task { @MainActor in self?.confirmed(phone) }
        }
    }

    /// What this Mac answers a paired phone's request with: Kuvo's own routes (`kuvo/…`), the
    /// coding agents' bridge's (`kuvo/agents/…`, at `agents`) and the engine's (at `base`).
    nonisolated static func respond(to request: LinkRequest, link: PhoneLink?, engine base: URL, password: String?,
                                    agents: OutsideAgents.Address?) -> AsyncThrowingStream<LinkResponsePart, Error> {
        guard request.path.hasPrefix("kuvo/") else {
            return Self.namingAgents(in: request, by: link) { EngineRoutes.forward($0, to: base, password: password) }
        }
        // The outside agents' sessions, through their bridge, as the engine's go through it.
        if request.path.hasPrefix(Self.agentsPrefix) {
            var forwarded = request
            forwarded.path = String(request.path.dropFirst(Self.agentsPrefix.count))
            guard let bridge = agents?.endpoint else { return Self.unavailable() }
            let base = bridge.url.absoluteString.hasSuffix("/") ? bridge.url : URL(string: bridge.url.absoluteString + "/")!
            // Their accounts and Auto too; never where the engine is (the Mac says).
            return Self.namingAgents(in: forwarded, by: link) {
                EngineRoutes.forward($0, to: base, password: bridge.password, allowing: { EngineRoutes.allowsAgents($0, method: $1) })
            }
        }
        if request.path == "kuvo/asks/events" { return Self.following { link?.asksJSON() } }
        if request.path == "kuvo/groups/events" { return Self.following { link?.groupsJSON() } }
        if let id = Self.groupID(in: request.path, then: "events") { return Self.followingGroup(id, link: link) }
        // A sign-in's page that sent the browser back to this Mac, loaded here.
        if request.path == "kuvo/sign-in/return" {
            return AsyncThrowingStream { continuation in
                let task = Task {
                    let (status, body) = request.method.uppercased() == "POST" ? await Self.loadSignInReturn(request.body) : (405, Data())
                    continuation.yield(.head(status: status, headers: ["content-type": "application/json"]))
                    continuation.yield(.body(body))
                    continuation.finish()
                }
                continuation.onTermination = { _ in task.cancel() }
            }
        }
        if request.path == "kuvo/usage/events" { return Self.following { link?.usageJSON() } }
        // What a phone does in a group chat: one on a machine answers once the machine has.
        if request.path.hasPrefix("kuvo/groups/"), request.method.uppercased() == "POST" {
            return AsyncThrowingStream { continuation in
                Task { @MainActor in
                    let (status, body) = await link?.answerGroup(String(request.path.dropFirst("kuvo/groups/".count)), with: request.body) ?? (503, Data())
                    continuation.yield(.head(status: status, headers: ["content-type": "application/json"]))
                    continuation.yield(.body(body))
                    continuation.finish()
                }
            }
        }
        // This Mac's sign-ins, listed (`api/credential` has what each is signed in with too).
        if request.path.split(separator: "?").first == "kuvo/accounts/credentials" {
            return AsyncThrowingStream { continuation in
                Task { @MainActor in
                    let (status, body) = await link?.credentialsJSON() ?? (503, Data())
                    continuation.yield(.head(status: status, headers: ["content-type": "application/json"]))
                    continuation.yield(.body(body))
                    continuation.finish()
                }
            }
        }
        // A session's wake-ups and watches: read, or one canceled.
        if request.path.hasPrefix("kuvo/wakes/") {
            return AsyncThrowingStream { continuation in
                Task { @MainActor in
                    let (status, body) = await link?.answerWakes(request) ?? (503, Data())
                    continuation.yield(.head(status: status, headers: ["content-type": "application/json"]))
                    continuation.yield(.body(body))
                    continuation.finish()
                }
            }
        }
        if let id = Self.shellID(in: request.path), request.method.uppercased() == "POST" {
            return AsyncThrowingStream { continuation in
                Task { @MainActor in
                    let (status, body) = await link?.stopShell(id, with: request.body) ?? (503, Data())
                    continuation.yield(.head(status: status, headers: ["content-type": "application/json"]))
                    continuation.yield(.body(body))
                    continuation.finish()
                }
            }
        }
        return AsyncThrowingStream { continuation in
            Task { @MainActor in
                let (status, body) = link?.answer(request) ?? (503, Data())
                continuation.yield(.head(status: status, headers: ["content-type": "application/json"]))
                continuation.yield(.body(body))
                continuation.finish()
            }
        }
    }

    /// A phone's session proved its own: its notifications go where it is now, for machines'
    /// bridges too.
    private func confirmed(_ phone: LinkServer.Phone) {
        let encoded = phone.key.base64URL
        guard let index = phones.firstIndex(where: { $0.key == encoded }), phones[index].address != phone.address.text else { return }
        phones[index].address = phone.address.text
        save()
        syncMachines()
    }

    /// Lets in a phone you've paired, or one showing the pairing link's token (once, in time).
    private func authorize(_ key: Data, _ address: LinkAddress, _ hello: PhoneHello) -> Bool {
        let encoded = key.base64URL
        // Where a paired one's notifications go changes once its session proves its own (see
        // `confirmed`): the first handshake message can be replayed from another address.
        if phones.contains(where: { $0.key == encoded }) { return true }
        guard let pairing, ContinuousClock.now < pairingEnds, let token = hello.pair.flatMap({ Data(base64URL: $0) }),
              token.count == pairing.token.count, zip(token, pairing.token).reduce(0, { $0 | ($1.0 ^ $1.1) }) == 0
        else { return false }
        phones.append(Phone(key: encoded, name: String(hello.name.prefix(100)), added: .now, address: address.text))
        save()
        self.pairing = nil
        syncMachines()
        log.info("Paired a phone")
        return true
    }

    // MARK: Notifications

    /// Sends `notice` to your phones' notifications, while you're away from this Mac or have the
    /// app open on a phone (which leaves out what's on its screen): at the Mac, its own
    /// notifications are enough. News from `machine`'s engine goes only while that machine's
    /// bridge knows this Mac is here; otherwise the bridge has told it.
    func push(_ notice: Notice, from machine: String? = nil) {
        guard Self.isEnabled, let server, let data = try? notice.encoded() else { return }
        if let machine, !(markedPresent[machine].map { .now - $0 < Self.presentFor } ?? false) { return }
        let phones = phones.compactMap { phone -> (key: Data, address: LinkAddress)? in
            guard let key = Data(base64URL: phone.key), let address = phone.address.flatMap(LinkAddress.init(text:)) else { return nil }
            return (key, address)
        }
        guard !phones.isEmpty else { return }
        let away = Self.isAwayFromMac
        Task {
            let isOpen = await server.hasSessions
            guard away || isOpen else { return }
            await server.push(data, to: phones)
        }
    }

    /// No one has used this Mac for two minutes, or its screen is locked.
    private static var isAwayFromMac: Bool {
        let idle = CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: CGEventType(rawValue: ~0)!)
        let locked = (CGSessionCopyCurrentDictionary() as? [String: Any])?["CGSSessionScreenIsLocked"] as? Bool ?? false
        return idle >= 120 || locked
    }

    /// Where the outside agents' bridge is reached over the link.
    nonisolated static let agentsPrefix = "kuvo/agents/"

    /// A message the phone sends (`POST api/session/<id>/prompt`) says who the agents it names
    /// with "&" are, as one written here does (`AgentReferences`), before `forward` takes it on.
    nonisolated private static func namingAgents(in request: LinkRequest, by link: PhoneLink?,
                                                 forward: @escaping @Sendable (LinkRequest) -> AsyncThrowingStream<LinkResponsePart, Error>) -> AsyncThrowingStream<LinkResponsePart, Error> {
        let parts = request.path.split(separator: "?").first?.split(separator: "/") ?? []
        guard request.method.uppercased() == "POST", parts.count == 4, parts[0] == "api", parts[1] == "session", parts[3] == "prompt",
              let session = String(parts[2]).removingPercentEncoding, request.body.contains(UInt8(ascii: "&")) else { return forward(request) }
        return AsyncThrowingStream { continuation in
            let task = Task { @MainActor in
                var named = request
                if var body = (try? JSONSerialization.jsonObject(with: request.body)) as? [String: Any], let text = body["text"] as? String {
                    let directory = link?.workspace?.session(session)?.location.directory
                    body["text"] = AgentReferences.naming(in: text, excluding: session, here: directory.flatMap(RemoteFolder.machine(of:)))
                    if let data = try? JSONSerialization.data(withJSONObject: body) { named.body = data }
                }
                do {
                    for try await part in forward(named) { continuation.yield(part) }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    nonisolated private static func unavailable() -> AsyncThrowingStream<LinkResponsePart, Error> {
        AsyncThrowingStream { continuation in
            continuation.yield(.head(status: 503, headers: ["content-type": "application/json"]))
            continuation.yield(.body(Data(#"{"message":"The coding agents aren't running on this Mac."}"#.utf8)))
            continuation.finish()
        }
    }

    /// Kuvo's own routes, beside the engine's.
    private func answer(_ request: LinkRequest) -> (Int, Data) {
        /// `chats`: the folder of conversations of no project, which the phone lists as Chats.
        /// `agents`: the outside agents' bridge is up, at `kuvo/agents/api/…`.
        struct Info: Encodable {
            var kind = "mac"; var name: String; var app: String; var engine: String; var chats: String
            var agents = false
            /// Integrations' names by their MCP server's, for what a server asks.
            var integrations = Dictionary(uniqueKeysWithValues: Integrations.all.map { ($0.id, $0.title) })
        }
        struct Hosts: Encodable {
            /// `machine`: its id, as bots name theirs. `anotherMac`: its engine is another Mac's,
            /// which gives it its settings, so this Mac's (`kuvo/config`) don't reach it.
            struct Host: Encodable {
                var kind = "machine"; var name: String; var address: String; var key: String; var relay: String
                var machine: String; var anotherMac: Bool
            }
            var hosts: [Host]
        }
        let encoder = JSONEncoder()
        switch request.path {
        case "kuvo/info":
            var info = Info(name: Self.computerName, app: AppInfo.version, engine: OpenCodeEngine.version, chats: Workspace.chatsFolder)
            info.agents = agents?.api != nil
            return (200, (try? encoder.encode(info)) ?? Data())
        case "kuvo/bots":
            // Each bot by its session (here or on a machine): its name, look and job, and what
            // the Mac knows of it that its session doesn't say: paused, when it next wakes (in
            // seconds since 1970), how many things it watches.
            struct Listed: Encodable {
                struct Entry: Encodable {
                    var session: String, name: String, look: BotLook, machine: String?, job: String
                    var paused: Bool, wakes: Double?, watching: Int
                    /// Its conversation is with the coding agents: read through `kuvo/agents/api/…`.
                    var agents: Bool
                }
                var bots: [Entry]
            }
            let entries = (bots?.list ?? []).map { bot in
                Listed.Entry(
                    session: bot.sessionID, name: bot.name, look: bot.look, machine: bot.machine, job: bot.job,
                    paused: bots?.paused.contains(bot.id) == true,
                    wakes: bots?.wakes[bot.id]?.map(\.due).min()?.timeIntervalSince1970,
                    watching: bots?.watches[bot.id]?.count ?? 0,
                    agents: bot.agents == true)
            }
            return (200, (try? encoder.encode(Listed(bots: entries))) ?? Data())
        case "kuvo/named":
            // Who "&" can name in a message: every bot, and the recent sessions with a name.
            struct Named: Encodable {
                struct Entry: Encodable { var session: String, name: String, kind: String, machine: String?, project: String?, about: String? }
                var agents: [Entry]
            }
            let entries = AgentReferences.candidates(excluding: nil).map {
                Named.Entry(session: $0.session, name: $0.name, kind: $0.kind, machine: $0.machine, project: $0.project, about: $0.about)
            }
            return (200, (try? encoder.encode(Named(agents: entries))) ?? Data())
        case "kuvo/hosts":
            let hosts = machineHosts.sorted { $0.value.name < $1.value.name }.map { id, host in
                Hosts.Host(name: host.name, address: host.address, key: host.key, relay: relay.absoluteString, machine: id, anotherMac: isAnotherMacs(id))
            }
            return (200, (try? encoder.encode(Hosts(hosts: hosts))) ?? Data())
        case "kuvo/asks":
            return (200, asksJSON())
        case "kuvo/wakes":
            // Every session's with a wake-up or a watch, for the marks beside it as in the sidebar:
            // its soonest wake-up (seconds since 1970) and whether it watches. Bots' come with `kuvo/bots`.
            struct Listed: Encodable {
                struct Entry: Encodable { var session: String; var next: Double?; var watching: Bool }
                var sessions: [Entry]
            }
            let wakes = wakeUps?.wakes ?? [:]
            let watches = wakeUps?.watches ?? [:]
            let keys = Set(wakes.filter { !$0.value.isEmpty }.keys).union(watches.filter { !$0.value.isEmpty }.keys).filter { bots?.bot($0) == nil }
            let entries = keys.sorted().map { key in
                Listed.Entry(session: key, next: wakes[key]?.map(\.due).min()?.timeIntervalSince1970, watching: watches[key]?.isEmpty == false)
            }
            return (200, (try? JSONEncoder().encode(Listed(sessions: entries))) ?? Data())
        case "kuvo/groups":
            return (200, groupsJSON())
        case "kuvo/accounts/ended":
            return (200, (try? JSONEncoder().encode(["providers": endedSignIns()])) ?? Data())
        case let path where path.hasPrefix("kuvo/accounts/") && path.hasSuffix("/forget") && request.method.uppercased() == "POST":
            // Disconnect, as Settings › Accounts does it: its sign-ins and keys here, and the copies on your machines.
            let provider = String(path.dropFirst("kuvo/accounts/".count).dropLast("/forget".count))
            guard !provider.isEmpty, !provider.contains("/"), provider.range(of: #"^[A-Za-z0-9_@.-]+$"#, options: .regularExpression) != nil else { return (404, Data()) }
            return forgetSignIn(provider) ? (200, Data("{}".utf8)) : (404, Data())
        case "kuvo/usage":
            guard let usage = usageJSON() else { return (404, Data()) }
            return (200, usage)
        case "kuvo/usage/refresh" where request.method == "POST":
            #if DEBUG
            if Self.usageSample { return (200, Data("{}".utf8)) }
            #endif
            return UsageMenuBar.shared.refreshAll() ? (200, Data("{}".utf8)) : (404, Data())
        case let path where path.hasPrefix("kuvo/asks/") && request.method == "POST":
            return answerAsk(String(path.dropFirst("kuvo/asks/".count)), with: request.body)
        case "kuvo/config":
            return answerConfig(request)
        case "kuvo/mcp/catalog":
            return catalogJSON()
        case let path where path.hasPrefix("kuvo/mcp/catalog/") && request.method.uppercased() == "POST":
            return addFromCatalog(String(path.dropFirst("kuvo/mcp/catalog/".count)))
        case "kuvo/engine/restart" where request.method.uppercased() == "POST":
            // Answered first: the restart takes the link down with the engine, and the phone sees
            // this Mac come back (`kuvo/info` answers again).
            let restart = restartEngine
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(1))
                restart()
            }
            return (202, Data("{}".utf8))
        default:
            return (404, Data())
        }
    }

    // MARK: Settings

    /// `kuvo/config`: this Mac's engine settings, as Settings shows them (your machines get them
    /// too, but for what only makes sense here). PATCH changes some, as Settings does, with
    /// `{"changes": [{"path": [keys…], "value": …}]}` (null removes it), and answers with them all.
    /// A change can say what it read there (`"was"`, null for nothing): changed since, nothing is
    /// changed and it answers 409.
    private func answerConfig(_ request: LinkRequest) -> (Int, Data) {
        guard let config = engineConfig else { return (503, Data()) }
        // As the file is now, edited elsewhere or not.
        config.reload()
        if let problem = config.problem { return (409, Self.message("The engine's settings file can't be changed from here: \(problem)")) }
        switch request.method.uppercased() {
        case "GET":
            break
        case "PATCH":
            struct Change: Decodable {
                var path: [String]
                var value: JSONValue
                /// Absent: not checked; `.null`: nothing was there.
                var was: JSONValue?

                enum CodingKeys: String, CodingKey { case path, value, was }
                init(from decoder: Decoder) throws {
                    let container = try decoder.container(keyedBy: CodingKeys.self)
                    path = try container.decode([String].self, forKey: .path)
                    value = try container.decode(JSONValue.self, forKey: .value)
                    was = container.contains(.was) ? try container.decode(JSONValue.self, forKey: .was) : nil
                }
            }
            struct Given: Decodable { var changes: [Change] }
            guard let changes = (try? JSONDecoder().decode(Given.self, from: request.body))?.changes, (1...100).contains(changes.count),
                  changes.allSatisfy({ (1...16).contains($0.path.count) && $0.path.allSatisfy { !$0.isEmpty && $0.count <= 200 } })
            else { return (400, Self.message("Send {\"changes\": [{\"path\": [keys…], \"value\": …}]}.")) }
            if changes.contains(where: { change in change.was.map { (config.value(at: change.path) ?? .null) != $0 } ?? false }) {
                return (409, Self.message("It changed since you read it."))
            }
            config.set(changes.map { (path: $0.path, value: $0.value == .null ? nil : $0.value) })
            if let problem = config.problem { return (500, Self.message(problem)) }
        default:
            return (405, Data())
        }
        return (200, Self.settingsJSON(config.settings))
    }

    private static func settingsJSON(_ settings: [String: JSONValue]) -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return (try? encoder.encode(settings)) ?? Data("{}".utf8)
    }

    nonisolated private static func message(_ text: String) -> Data {
        (try? JSONEncoder().encode(["message": text])) ?? Data()
    }

    /// `kuvo/mcp/catalog`: Settings › MCP Servers' popular servers, each with whether it's added
    /// (`integration` too when Integrations added it, and keeps it there), and `macOnly` for one
    /// that runs on this Mac alone (its Chrome), which never reaches your machines.
    private func catalogJSON() -> (Int, Data) {
        guard let config = engineConfig else { return (503, Data()) }
        struct Listed: Encodable {
            struct Entry: Encodable {
                var id: String, title: String, summary: String, access: String
                var added: Bool, integration: Bool, macOnly: Bool
            }
            var servers: [Entry]
        }
        config.reload()
        let entries = MCPCatalog.servers.map { server in
            let added = config.value(at: MCPSettings.path + [server.id]) != nil
            let access = switch server.access {
            case .signIn: "signIn"
            case .open: "open"
            case .local: "local"
            }
            return Listed.Entry(id: server.id, title: server.title, summary: server.summary, access: access, added: added,
                                integration: added && Integrations.service(server.id) != nil, macOnly: MCPCatalog.macOnly.contains(server.id))
        }
        return (200, (try? JSONEncoder().encode(Listed(servers: entries))) ?? Data())
    }

    /// `POST kuvo/mcp/catalog/<id>`: adds one of them, as its Add button does, and answers with
    /// the settings (as `kuvo/config`). One already added stays as it is.
    private func addFromCatalog(_ id: String) -> (Int, Data) {
        guard let config = engineConfig else { return (503, Data()) }
        guard let server = MCPCatalog.servers.first(where: { $0.id == id }) else { return (404, Data()) }
        config.reload()
        if let problem = config.problem { return (409, Self.message("The engine's settings file can't be changed from here: \(problem)")) }
        if config.value(at: MCPSettings.path + [server.id]) == nil { MCPCatalog.add(server, to: config) }
        if let problem = config.problem { return (500, Self.message(problem)) }
        return (200, Self.settingsJSON(config.settings))
    }

    /// `POST kuvo/sign-in/return` with `{"url"}`: where a sign-in's page sent the browser back
    /// to this Mac (an MCP server's, or a plan's that finishes here), which a phone can't load,
    /// loaded here, as the browser would have. Only this Mac's own address
    /// (`EngineRoutes.signInReturn`), and no further than it answers (a redirect isn't followed).
    /// Answers with the status it got (`{"status"}`): 2xx or 3xx, it took it.
    nonisolated private static func loadSignInReturn(_ body: Data) async -> (Int, Data) {
        struct Given: Decodable { var url: String }
        guard let given = try? JSONDecoder().decode(Given.self, from: body), let url = EngineRoutes.signInReturn(given.url) else {
            return (400, message("That isn't an address a sign-in returns to on this computer."))
        }
        var request = URLRequest(url: url)
        request.timeoutInterval = 15
        guard let (_, response) = try? await URLSession.shared.data(for: request, delegate: StaysThere()),
              let status = (response as? HTTPURLResponse)?.statusCode else {
            return (502, message("Nothing on this computer took it. Start the sign-in again."))
        }
        return (200, (try? JSONEncoder().encode(["status": status])) ?? Data())
    }

    // MARK: Accounts

    /// `kuvo/accounts/credentials`: each sign-in this Mac's engine keeps, by its ID, with the plan
    /// it's for, its name and whether it's the one in use: the list alone (the engine's
    /// `api/credential` has what each is signed in with too).
    private func credentialsJSON() async -> (Int, Data) {
        guard let api = workspace?.api else { return (503, Data()) }
        guard let credentials = try? await api.credentials() else { return (502, Data()) }
        struct Listed: Encodable { var id: String; var integrationID: String; var label: String; var active: Bool }
        let listed = credentials.map { Listed(id: $0.id, integrationID: $0.integrationID, label: $0.label, active: $0.active) }
        return (200, (try? JSONEncoder().encode(["credentials": listed])) ?? Data())
    }

    // MARK: Asks

    /// What agents ask you for while they browse, waiting now: a sign-in (with the fields the
    /// page has, and the accounts Passwords keeps for the site, never their passwords), a code,
    /// or a step only you can do.
    private func asksJSON() -> Data {
        struct Listed: Encodable {
            struct Ask: Encodable {
                var id: String
                var kind: String
                /// The page's own site.
                var host: String
                /// The machine whose browser it is, by name; none for this Mac's.
                var machine: String?
                var session: String?
                var asker: String?
                var username: Bool?
                var password: Bool?
                var saved: [String]?
                /// Whether what's typed is kept in Passwords unless you say otherwise.
                var offersToSave: Bool?
                var prompt: String?
                var instruction: String?
            }
            var asks: [Ask]
        }
        let asks = (browserAsks?.asks ?? []).map { ask in
            var listed = Listed.Ask(id: ask.id, kind: "", host: ask.host, machine: ask.machine.map { machineName($0) ?? $0 }, session: ask.session, asker: ask.asker)
            switch ask.kind {
            case .signIn(let username, let password):
                listed.kind = "signIn"
                listed.username = username
                listed.password = password
                let saved = Passwords.logins(for: ask.host).map(\.account)
                listed.saved = saved
                listed.offersToSave = saved.isEmpty && Passwords.offersToSave && !Passwords.neverSites.contains(ask.host)
            case .code(let prompt):
                listed.kind = "code"
                listed.prompt = prompt
            case .action(let instruction):
                listed.kind = "action"
                listed.instruction = instruction
            }
            return listed
        }
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        return (try? encoder.encode(Listed(asks: asks))) ?? Data()
    }

    /// A phone's answer, as the card's: a sign-in typed there (kept in Passwords if asked) or one
    /// of the site's saved ones, which this Mac fills in itself; a code; done; or no.
    private func answerAsk(_ id: String, with body: Data) -> (Int, Data) {
        struct Given: Decodable {
            var answer: String
            var username: String?
            var password: String?
            /// An account Passwords keeps for the site: its password, from here.
            var saved: String?
            var save: Bool?
            var code: String?
        }
        guard let asks = browserAsks, let ask = asks.asks.first(where: { $0.id == id }) else { return (404, Data()) }
        guard let given = try? JSONDecoder().decode(Given.self, from: body) else { return (400, Data()) }
        switch (given.answer, ask.kind) {
        case ("signIn", .signIn):
            if let account = given.saved {
                guard let login = Passwords.logins(for: ask.host).first(where: { $0.account == account }),
                      let password = Passwords.password(for: login) else { return (404, Data()) }
                asks.answer(id, .signIn(username: account.isEmpty ? nil : account, password: password))
            } else {
                let username = given.username.flatMap { $0.isEmpty ? nil : $0 }
                let password = given.password.flatMap { $0.isEmpty ? nil : $0 }
                guard username != nil || password != nil else { return (400, Data()) }
                if given.save == true, let password { _ = Passwords.save(host: ask.host, account: username ?? "", password: password) }
                asks.answer(id, .signIn(username: username, password: password))
            }
        case ("code", .code):
            guard let code = given.code?.trimmingCharacters(in: .whitespaces), !code.isEmpty else { return (400, Data()) }
            asks.answer(id, .code(code))
        case ("done", .action):
            asks.answer(id, .done)
        case ("declined", _):
            asks.answer(id, .declined)
        default:
            return (400, Data())
        }
        return (200, Data("{}".utf8))
    }

    // MARK: Usage

    /// The menu bar's usage panel, as its Bars view lists it; nil while it isn't running (a phone
    /// then hides Usage).
    private func usageJSON() -> Data? {
        #if DEBUG
        if Self.usageSample { return Data(Self.sampleUsage.utf8) }
        #endif
        return UsageMenuBar.shared.phoneUsage()
    }

    #if DEBUG
    /// KUVO_DEMO_USAGE=sample: the usage tracker stays out (as with `off`), and phones get these
    /// numbers instead of your accounts'.
    static var usageSample: Bool { ProcessInfo.processInfo.environment["KUVO_DEMO_USAGE"] == "sample" }

    private static var sampleUsage: String {
        let now = Int(Date.now.timeIntervalSince1970)
        // The providers' own marks, as the real panel sends them.
        let icon = { (id: String) in UsageMenuBar.phoneMark(for: id).map { #""icon":"\#($0)","iconIsTemplate":true,"# } ?? "" }
        return """
        {"providers":[\
        {"id":"claude",\(icon("claude"))"name":"Claude","plan":"Max","color":"#DE7356","refreshed":\(now - 40),"lines":[\
        {"type":"progress","label":"Session","used":42,"limit":100,"format":{"kind":"percent"},"resetsAt":\(now + 7_200),"periodDurationMs":18000000},\
        {"type":"progress","label":"Weekly","used":67,"limit":100,"format":{"kind":"percent"},"resetsAt":\(now + 259_200),"periodDurationMs":604800000}]},\
        {"id":"codex",\(icon("codex"))"name":"Codex","plan":"Pro","color":"#10A37F","refreshed":\(now - 95),"lines":[\
        {"type":"progress","label":"Session","used":18,"limit":100,"format":{"kind":"percent"},"resetsAt":\(now + 12_600),"periodDurationMs":18000000},\
        {"type":"text","label":"Credits","value":"$12.40"}]},\
        {"id":"cursor",\(icon("cursor"))"name":"Cursor","color":"#13120A","refreshed":\(now - 900),"notice":"Not logged in. Sign in to Cursor to see its usage.","stale":true,"lines":[\
        {"type":"badge","label":"Error","text":"Not logged in"}]}],\
        "refreshing":false,"refreshed":\(now - 40)}
        """
    }
    #endif

    // MARK: Group chats

    /// A chat's bots as a phone draws them: their face and name, and where each lives.
    private struct GroupMember: Encodable {
        var id: String
        var name: String
        var look: BotLook
        var session: String
        var directory: String
        /// Its own conversation is with the coding agents.
        var agents: Bool
    }

    private func members(of group: BotGroup) -> [GroupMember] {
        group.members.compactMap { bots?.bot($0) }.map { GroupMember(id: $0.id, name: $0.name, look: $0.look, session: $0.sessionID, directory: $0.directory, agents: $0.agents == true) }
    }

    private static var groupEncoder: JSONEncoder {
        let encoder = JSONEncoder()
        // Sorted, as a stream sends what it's encoded only when the text differs, and JSONEncoder
        // writes a struct's keys in another order almost every time.
        encoder.outputFormatting = .sortedKeys
        encoder.dateEncodingStrategy = .secondsSince1970
        return encoder
    }

    /// Your group chats as the sidebar lists them: who's in each, who's talking, whether it
    /// waits on you to keep going, and the last thing said.
    private func groupsJSON() -> Data {
        struct Listed: Encodable {
            struct Group: Encodable {
                var id: String
                var name: String
                var purpose: String?
                var members: [GroupMember]
                var speaking: String?
                var discussing: Bool
                var paused: Bool
                var unread: Bool
                var last: GroupMessage?
                var meeting: BotGroup.Meeting?
                /// The machine it lives on (its bots' too), by id as bots name theirs; none for this Mac.
                var machine: String?
                /// On its way to another computer: what would change it waits until it's there.
                var moving: Bool
            }
            var groups: [Group]
        }
        guard let groups else { return Data(#"{"groups":[]}"#.utf8) }
        let listed = groups.list.map { group in
            var last = groups.messages(in: group.id).last { $0.kind != .note }
            last?.steps = nil
            return Listed.Group(id: group.id, name: group.name, purpose: group.purpose, members: members(of: group),
                                speaking: groups.speaking[group.id], discussing: groups.discussing.contains(group.id),
                                paused: groups.paused.contains(group.id), unread: groups.isUnread(group), last: last, meeting: group.meeting,
                                machine: groups.machine(of: group.id), moving: isMoving(group.id))
        }
        return (try? Self.groupEncoder.encode(Listed(groups: listed))) ?? Data()
    }

    /// One chat as it reads now: its latest messages (with what each bot did on its turn), who's
    /// talking, and whether it's under way or paused. Nil once the chat's gone.
    private func groupJSON(_ id: String) -> Data? {
        groupLines(id)?.whole
    }

    /// One chat's line, whole and without its messages (`state`), for `followingGroup`.
    struct GroupLines: Equatable {
        var messages: Data
        var state: Data
        /// `state` with its `messages` first.
        var whole: Data { Data(#"{"messages":"#.utf8) + messages + Data(",".utf8) + state.dropFirst() }
    }

    private func groupLines(_ id: String) -> GroupLines? {
        struct Chat: Encodable {
            var speaking: String?
            var discussing: Bool
            var paused: Bool
            var moving: Bool
            /// What the bot talking is doing, its last step (see `GroupActivity`).
            var activity: [GroupActivity]?
        }
        guard let groups, groups.group(id) != nil else { return nil }
        let chat = Chat(speaking: groups.speaking[id], discussing: groups.discussing.contains(id), paused: groups.paused.contains(id),
                        moving: isMoving(id), activity: groups.activity[id])
        guard let messages = try? Self.groupEncoder.encode(Array(groups.messages(in: id).suffix(300))),
              let state = try? Self.groupEncoder.encode(chat) else { return nil }
        return GroupLines(messages: messages, state: state)
    }

    /// What a phone does in a chat, as in the window: writes to it (`{"text"}`), stops it, lets it
    /// keep going, looks at it, takes a message of yours back (`{"message"}`, answering with
    /// what was taken, for the phone's composer), or starts the work a wrap-up split up
    /// (`{"message"}`, the wrap-up).
    private func answerGroup(_ path: String, with body: Data) async -> (Int, Data) {
        let parts = path.split(separator: "/").map(String.init)
        guard let groups, let id = parts.first, groups.group(id) != nil else { return (404, Data()) }
        struct Given: Decodable { var text: String?; var message: String? }
        let given = try? JSONDecoder().decode(Given.self, from: body)
        switch parts.dropFirst().first {
        case nil:
            guard let text = given?.text, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return (400, Data()) }
            // Moving, or its machine away or saying no: the phone keeps it, and says so.
            guard await groups.sendNow(text, to: id) else { return (409, Data(#"{"message":"It can't take messages right now. Try again in a moment."}"#.utf8)) }
        case "stop":
            groups.stop(id)
        case "keep-going":
            groups.keepGoing(id)
        case "viewed":
            groups.markViewed(id)
        case "undo":
            guard let message = given?.message else { return (404, Data()) }
            // Moving, or its machine away: as for what you write.
            guard !isMoving(id), groups.unreachable(id) == nil else { return (409, Data()) }
            guard let taken = await groups.undo(from: message, in: id) else { return (404, Data()) }
            return (200, (try? JSONEncoder().encode(["text": taken])) ?? Data())
        case "start":
            guard let message = given?.message, groups.messages(in: id).contains(where: { $0.id == message && $0.tasks != nil }) else { return (404, Data()) }
            groups.startWork(message, in: id)
        default:
            return (404, Data())
        }
        return (200, Data("{}".utf8))
    }

    /// The chat in `kuvo/groups/<id>/<action>`.
    /// `kuvo/shell/<id>/stop` → the engine's id of the command.
    nonisolated private static func shellID(in path: String) -> String? {
        let parts = path.split(separator: "/")
        guard parts.count == 4, parts[0] == "kuvo", parts[1] == "shell", parts[3] == "stop" else { return nil }
        return String(parts[2]).removingPercentEncoding
    }

    /// `GET kuvo/wakes/<session>`: its wake-ups, watches and whether they're paused, read now;
    /// `POST kuvo/wakes/<session>/cancel` with `{"wake": "<id>"}` or `{"watch": "<letters>"}`
    /// cancels one, and `…/now` with `{"wake": "<id>"}` sends one now (one that repeats still
    /// comes back at its time), as the bar over the Mac's composer does. Times are seconds since 1970.
    private func answerWakes(_ request: LinkRequest) async -> (Int, Data) {
        let parts = request.path.split(separator: "?").first?.split(separator: "/").map(String.init) ?? []
        guard parts.count >= 3, let session = parts[2].removingPercentEncoding, let app = AgentMail.shared.app,
              let found = app.workspace.session(session) else { return (404, Data("{}".utf8)) }
        let owner = WakeUps.owner(of: found)
        let wakeUps = app.wakeUps
        // As they are now: a machine session's aren't read until something asks.
        await wakeUps.read(owner)
        if parts.count == 4, parts[3] == "now", request.method.uppercased() == "POST" {
            struct Given: Decodable { var wake: String }
            guard let id = (try? JSONDecoder().decode(Given.self, from: request.body))?.wake,
                  let wake = wakeUps.wakes[owner.key]?.first(where: { $0.id == id || $0.letters == id }) else { return (404, Data("{}".utf8)) }
            do { try await wakeUps.wakeNow(wake, of: owner) } catch {
                return (503, (try? JSONEncoder().encode(["message": error.localizedDescription])) ?? Data())
            }
            return (200, Data("{}".utf8))
        }
        if parts.count == 4, parts[3] == "cancel", request.method.uppercased() == "POST" {
            struct Given: Decodable { var wake: String?; var watch: String? }
            let given = try? JSONDecoder().decode(Given.self, from: request.body)
            do {
                if let id = given?.wake, let wake = wakeUps.wakes[owner.key]?.first(where: { $0.id == id || $0.letters == id }) {
                    try await wakeUps.cancel(wake, of: owner)
                } else if let letters = given?.watch, let watch = wakeUps.watches[owner.key]?.first(where: { $0.letters == letters }) {
                    try await wakeUps.cancel(watch, of: owner)
                } else {
                    return (404, Data("{}".utf8))
                }
            } catch {
                return (503, (try? JSONEncoder().encode(["message": error.localizedDescription])) ?? Data())
            }
            return (200, Data("{}".utf8))
        }
        guard parts.count == 3 else { return (404, Data("{}".utf8)) }
        struct Listed: Encodable {
            struct Wake: Encodable { var id: String; var due: Double; var note: String; var every: String? }
            struct Watch: Encodable { var letters: String; var every: String; var check: String; var note: String }
            var wakes: [Wake]; var watches: [Watch]; var paused: Bool
        }
        let listed = Listed(wakes: (wakeUps.wakes[owner.key] ?? []).map { .init(id: $0.id, due: $0.due.timeIntervalSince1970, note: $0.note, every: $0.every?.text) },
                            watches: (wakeUps.watches[owner.key] ?? []).map { .init(letters: $0.letters, every: $0.every, check: $0.check, note: $0.note) },
                            paused: wakeUps.paused.contains(owner.key))
        return (200, (try? JSONEncoder().encode(listed)) ?? Data())
    }

    /// Stops a command an agent left running, as Stop in the bar over the composer does: found
    /// among the ones this Mac follows, or asked of the engine of `session` when given.
    private func stopShell(_ id: String, with body: Data) async -> (Int, Data) {
        struct Given: Decodable { var session: String? }
        guard let workspace else { return (503, Data()) }
        var found = workspace.shellJobs.lazy.compactMap { session, jobs in jobs.first { $0.id == id }.map { (session, $0) } }.first
        if found == nil, let session = (try? JSONDecoder().decode(Given.self, from: body))?.session, let api = workspace.api(forSession: session),
           let job = try? await api.shell(id, directory: workspace.session(session)?.location.directory), job.status == "running" {
            found = (session, job)
        }
        guard let (session, job) = found else { return (404, Data("{}".utf8)) }
        let how = await workspace.stopShellJob(job, in: session)
        return (200, Data(#"{"stopped":"\#(how)"}"#.utf8))
    }

    nonisolated private static func groupID(in path: String, then action: String) -> String? {
        let parts = path.split(separator: "/")
        guard parts.count == 4, parts[0] == "kuvo", parts[1] == "groups", parts[3] == action else { return nil }
        return String(parts[2])
    }

    /// A response that doesn't end: `current()` now, as a line of JSON, and again each time it
    /// changes, until the phone stops reading. 404 when there's nothing to follow (no usage panel,
    /// no such chat), as reading it once answers.
    nonisolated private static func following(_ current: @escaping @MainActor () -> Data?) -> AsyncThrowingStream<LinkResponsePart, Error> {
        AsyncThrowingStream { continuation in
            let task = Task { @MainActor in
                guard current() != nil else {
                    continuation.yield(.head(status: 404, headers: [:]))
                    continuation.finish()
                    return
                }
                continuation.yield(.head(status: 200, headers: ["content-type": "application/x-ndjson"]))
                var last: Data?
                while !Task.isCancelled {
                    guard let now = current() else { break }
                    if now != last {
                        continuation.yield(.body(now + Data("\n".utf8)))
                        last = now
                    }
                    await changed(current)
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// One chat's events (`kuvo/groups/<id>/events`), as `following` sends them, but a line whose
    /// messages are as they were (who's talking, or what they're doing, changed) leaves them out,
    /// and the phone keeps the ones it has.
    nonisolated private static func followingGroup(_ id: String, link: PhoneLink?) -> AsyncThrowingStream<LinkResponsePart, Error> {
        AsyncThrowingStream { continuation in
            let task = Task { @MainActor in
                guard link?.groupLines(id) != nil else {
                    continuation.yield(.head(status: 404, headers: [:]))
                    continuation.finish()
                    return
                }
                continuation.yield(.head(status: 200, headers: ["content-type": "application/x-ndjson"]))
                var last: GroupLines?
                while !Task.isCancelled {
                    guard let now = link?.groupLines(id) else { break }
                    if now != last {
                        continuation.yield(.body((now.messages == last?.messages ? now.state : now.whole) + Data("\n".utf8)))
                        last = now
                    }
                    await changed { link?.groupLines(id)?.whole }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Returns once what `current` reads changes, or after half a minute.
    private static func changed(_ current: @escaping @MainActor () -> Data?) async {
        final class Once: @unchecked Sendable {
            var continuation: CheckedContinuation<Void, Never>?
            func resume() {
                continuation?.resume()
                continuation = nil
            }
        }
        let once = Once()
        await withCheckedContinuation { continuation in
            once.continuation = continuation
            withObservationTracking { _ = current() } onChange: {
                Task { @MainActor in once.resume() }
            }
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(30))
                once.resume()
            }
        }
    }

    private func save() {
        try? JSONEncoder().encode(Saved(phones: phones, machines: machineHosts)).write(to: Self.file, options: .atomic)
    }

    // MARK: Keychain

    private static var service: String { (Bundle.main.bundleIdentifier ?? "run.kuvo.code") + ".phone-link" }

    /// The item, or nil when there's none; throws when the Keychain can't be read.
    private static func keychain(_ account: String) throws -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        switch status {
        case errSecSuccess: return result as? Data
        case errSecItemNotFound: return nil
        default: throw NSError(domain: NSOSStatusErrorDomain, code: Int(status))
        }
    }

    private static func setKeychain(_ data: Data, for account: String) {
        let base: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        if SecItemUpdate(base as CFDictionary, [kSecValueData as String: data] as CFDictionary) != errSecItemNotFound { return }
        var item = base
        item[kSecValueData as String] = data
        item[kSecAttrLabel as String] = "Kuvo Code phone link"
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        SecItemAdd(item as CFDictionary, nil)
    }
}

/// Stops at the first answer an address gives, rather than following where it sends the browser
/// next (`kuvo/sign-in/return`).
nonisolated private final class StaysThere: NSObject, URLSessionTaskDelegate, Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}
