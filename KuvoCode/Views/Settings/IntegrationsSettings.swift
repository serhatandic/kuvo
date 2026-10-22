import OpenCodeKit
import SwiftUI

/// Integrations: your mail and calendar, chats, socials, code and work tools, for the agent and
/// your bots, on each computer (each has its own, with its own keys and sign-ins). Each the best
/// way it has: its maker's own server (connect, and sign in once), a key you bring, or the site in
/// the panel's browser.
struct IntegrationsSettings: View {
    @Environment(AppModel.self) private var app
    @Environment(\.settingsBox) private var box
    @State private var live: [MCPServer] = []
    @State private var signingIn: IntegrationRef?
    @State private var addingKey: KeyRequest?
    /// The sign-ins the engine keeps, by integration: a server can answer without one (Google's
    /// do), then turn every call down.
    @State private var signedIn: Set<String> = []
    @State private var removing: Integrations.Service?

    private var config: EngineConfig { app.config(for: box) }
    private var api: OpenCodeAPI? { app.api(for: box) }

    /// Those this computer can run: a machine, not those that use a Mac's own apps.
    private var offered: [Integrations.Service] {
        box.machine == nil ? Integrations.all : Integrations.all.filter { !Integrations.macOnly.contains($0.id) }
    }

    /// Each kind, in order. KUVO_DEMO_INTEGRATION_KINDS=<kinds>: only those, for screenshots.
    private static var kinds: [Integrations.Kind] {
        #if DEBUG
        if let only = ProcessInfo.processInfo.environment["KUVO_DEMO_INTEGRATION_KINDS"] {
            return Integrations.Kind.allCases.filter { kind in only.split(separator: ",").contains { kind.rawValue.lowercased().hasPrefix($0.lowercased()) } }
        }
        #endif
        return Integrations.Kind.allCases
    }

    var body: some View {
        list
        .modifier(LiveMCPServers(servers: $live, api: { app.api(for: box) }, change: config.settings))
        .task(id: live) { await readSignIns() }
        .task {
            // Where sites are signed in, as their browsers' cookies say.
            while !Task.isCancelled {
                await SiteSignIns.shared.refresh(app)
                try? await Task.sleep(for: .seconds(4))
            }
        }
        .confirmationDialog("Remove \(removing?.title ?? "")?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), presenting: removing) { service in
            Button("Remove", role: .destructive) { remove(service) }
        } message: { _ in
            Text("The agent and your bots stop using it, and your sign-in or key goes with it.")
        }
        #if DEBUG
        // KUVO_DEMO_ADD_INTEGRATION=<id>: added, as Connect does.
        .onAppear {
            if let id = ProcessInfo.processInfo.environment["KUVO_DEMO_ADD_INTEGRATION"], let service = Integrations.service(id),
               config.value(at: MCPSettings.path + [id]) == nil { add(service) }
            // KUVO_DEMO_SIGN_IN_SITE=<id>: signing in to that service on its site, in the sheet.
            if let id = ProcessInfo.processInfo.environment["KUVO_DEMO_SIGN_IN_SITE"], let service = Integrations.service(id) {
                app.settingsTabs.open(service, on: box)
            }
            // KUVO_DEMO_CONNECTED=<id>: its sign-in, on its last step.
            if let id = ProcessInfo.processInfo.environment["KUVO_DEMO_CONNECTED"], let service = Integrations.service(id) {
                signingIn = IntegrationRef(id: "mcp_demo", service: service)
            }
        }
        #endif
        .settingsDetail(item: $signingIn, title: { _ in nil }) { ref in
            ConnectDetail(integrationID: ref.id, name: ref.service?.title, symbol: ref.service?.symbol, note: ref.service == nil ? "The agent can use it now." : "", highlights: highlights(ref.service)) {
                signingIn = nil
                Task {
                    if let api { for server in live where server.integrationID == ref.id { await app.reconnect(server.name, api: api, machine: box.machine) } }
                    await refresh()
                    await readSignIns()
                }
            }
        }
        .settingsDetail(item: $addingKey, title: { "Connect \($0.service.title)" }) { request in
            KeyEntry(service: request.service) { saved in
                addingKey = nil
                guard saved else { return }
                // The engine reads the key from its file as it reads its settings again, which
                // adding the integration has it do: its server connects again once it has.
                add(request.service)
                Task {
                    try? await Task.sleep(for: .seconds(2))
                    if let api { await app.reconnect(request.service.id, api: api, machine: box.machine) }
                    await refresh()
                }
            }
        }
    }

    private var list: some View {
        ScrollingSettingsForm {
            // Integrations live in the settings file: one that can't be changed changes nothing here.
            ConfigProblem()
            ForEach(Self.kinds, id: \.self) { kind in
                Section(kind.rawValue) {
                    ForEach(offered.filter { $0.kind == kind }) { service in
                        row(service)
                    }
                }
            }
        }
    }

    private func row(_ service: Integrations.Service) -> some View {
        let added = config.value(at: MCPSettings.path + [service.id]) != nil
        let places = added ? self.places(service) : siteSignIns(service)
        return SettingsRowLayout {
            HStack(alignment: .center, spacing: 12) {
                Image(systemName: service.symbol)
                    .font(.system(size: 15))
                    .foregroundStyle(.secondary)
                    .frame(width: 22)
                VStack(alignment: .leading, spacing: 2) {
                    // The tag flows on after the name, so a narrow row wraps the two as one line.
                    Text("\(service.title)  \(Text(Self.how(service.way)).font(.subheadline).foregroundStyle(.secondary))")
                    Text(service.summary)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                    if !places.isEmpty {
                        PlacesLine(places: places, waitsForCodeHelp: "Ask the agent to use \(service.title): the first time, it gives you a code to enter on Microsoft's page.")
                            .padding(.top, 2)
                    }
                }
            }
            .settingsText()
            HStack {
                switch service.way {
                case .browser:
                    Button(SiteSignIns.shared.isSignedIn(service.id, on: box.machine) == true ? "Open…" : "Sign In…") { withAnimation(.ui) { app.settingsTabs.open(service, on: box) } }
                        .help("Sign in to \(service.title) in the browser the agent and your bots use on \(box.machine == nil ? "this Mac" : app.name(of: box))")
                default:
                    if !added {
                        if case .officialApp = service.way {
                            Button("Add App…") { addingKey = KeyRequest(service: service) }
                        } else if !Integrations.keys(of: service.way).isEmpty {
                            Button("Add Key…") { addingKey = KeyRequest(service: service) }
                        } else {
                            Button("Connect") { add(service) }
                        }
                    } else {
                        let server = live.first { $0.name == service.id }
                        // What needs you, first.
                        if places.first?.state == .needsSignIn, let integrationID = server?.integrationID {
                            Button("Sign In…") { signingIn = IntegrationRef(id: integrationID, service: service) }
                        } else if places.contains(where: { $0.state == .failed }) {
                            Button("Try Again") { reconnect(service, where: places.filter { $0.state == .failed }) }
                        } else {
                            // Nothing needs you: say it's set up, beside its menu.
                            Label("Added", systemImage: "checkmark")
                                .font(.callout)
                                .foregroundStyle(.secondary)
                        }
                        Menu {
                            if let integrationID = server?.integrationID {
                                Button("Sign In Again…") { signingIn = IntegrationRef(id: integrationID, service: service) }
                            }
                            if !Integrations.keys(of: service.way).isEmpty {
                                Button(isApp(service) ? "Change App…" : "Change Key…") { addingKey = KeyRequest(service: service) }
                            }
                            Button("Reconnect") { reconnect(service, where: places) }
                            Divider()
                            Button("Remove…", role: .destructive) { removing = service }
                        } label: {
                            Image(systemName: "ellipsis")
                        }
                        .menuIndicator(.hidden)
                        .fixedSize()
                        .help("More")
                    }
                }
            }
        }
    }

    /// How it's doing on this computer.
    private func places(_ service: Integrations.Service) -> [Place] {
        let server = live.first { $0.name == service.id }
        var here = Place.state(server)
        if here == .connected, needsSignIn(service, server) { here = .needsSignIn }
        if here == .connected, waitsForCode(service) { here = .waitsForCode }
        return [Place(name: app.name(of: box), machine: box.machine, state: here)]
    }

    /// Whether a service the agent uses on its site is signed in on this computer, as far as its
    /// browser's cookies tell.
    private func siteSignIns(_ service: Integrations.Service) -> [Place] {
        guard case .browser = service.way, let signedIn = SiteSignIns.shared.isSignedIn(service.id, on: box.machine) else { return [] }
        return [Place(name: app.name(of: box), machine: box.machine, state: signedIn ? .signedIn : .signedOut)]
    }

    /// Connects it again on this computer.
    private func reconnect(_ service: Integrations.Service, where places: [Place]) {
        Task {
            if let api { await app.reconnect(service.id, api: api, machine: box.machine) }
            await refresh()
        }
    }

    /// What signing in to it gives you, for the sheet's last step.
    private func highlights(_ service: Integrations.Service?) -> [ConnectView.Highlight] {
        guard let service else { return [] }
        return [ConnectView.Highlight(symbol: "text.bubble", title: "Just ask", detail: "The agent and your bots use \(service.title)'s own tools when you mention it, not its website.")]
    }

    /// Whether it waits for you to sign in: as its server says, or, answering without a sign-in,
    /// as there's none kept. Microsoft 365's server signs in by itself, with a code, when the
    /// agent first uses it, and keeps the sign-in in its folder.
    private func needsSignIn(_ service: Integrations.Service, _ server: MCPServer?) -> Bool {
        guard let server else { return false }
        if server.status.status == "needs_auth" { return true }
        guard server.status.status == "connected", let integration = server.integrationID else { return false }
        switch service.way {
        case .official, .officialApp: return !signedIn.contains(integration)
        default: return false
        }
    }

    private func readSignIns() async {
        guard let entries = try? await api?.credentialEntries() else { return }
        signedIn = Set(entries.filter { $0.active && $0.isOAuth }.map(\.integrationID))
    }

    /// Microsoft 365, without its sign-in yet (on this Mac: a machine's isn't looked at).
    private func waitsForCode(_ service: Integrations.Service) -> Bool {
        guard box.machine == nil, let files = Integrations.signInFiles[service.id], let first = files.first else { return false }
        return !FileManager.default.fileExists(atPath: AppInfo.supportDirectory.appending(path: "integrations/\(service.id)/\(first)").path)
    }

    static func how(_ way: Integrations.Way) -> String {
        switch way {
        case .official: "Official"
        case .officialToken: "Official · your token"
        case .officialApp: "Official · your app"
        case .package(_, let keys, _, _, _, let macOnly): keys.isEmpty ? (macOnly ? "On this Mac" : "Open source") : "Open source · your key"
        case .browser: "In the browser"
        }
    }

    private func isApp(_ service: Integrations.Service) -> Bool {
        if case .officialApp = service.way { return true }
        return false
    }

    private func add(_ service: Integrations.Service) {
        guard config.problem == nil else { return }
        let path = MCPSettings.path + [service.id]
        // Added already (a new key or app for it): as you set it up (turned off, Code Mode, headers
        // you added), written again so the engine reads its keys again.
        let existing = config.value(at: path)
        guard let settings = existing ?? Integrations.settings(for: service, on: box.machine) else { return }
        config.set(settings, at: path)
        // Back again: what its removal left to do on this machine, out of reach then, isn't done.
        if let machine = box.machine { app.machines.dropWaiting(marked: Machines.integrationMark(service.id), on: machine) }
        if existing == nil { Analytics.shared.track("integration_added", ["integration": service.id, "machine": box.machine != nil]) }
    }

    /// Out of the engine's settings, with its keys, its sign-in and what its server kept.
    private func remove(_ service: Integrations.Service) {
        // Its keys and sign-in stay while it stays in a settings file that can't be changed.
        guard config.problem == nil else { return }
        let listed = live.first { $0.name == service.id }?.integrationID
        Task {
            // Its sign-in as the engine has it now (the page's last look may have missed it), asked
            // before it leaves the settings, and then the server with it.
            var signIn = listed
            if signIn == nil, let api { signIn = (try? await api.mcpServers())?.first { $0.name == service.id }?.integrationID }
            removeEntry(service)
            if let signIn { app.forgetSignIn(signIn, on: box) }
        }
    }

    /// Out of this computer's settings, with its keys there and what its server kept there.
    private func removeEntry(_ service: Integrations.Service) {
        config.set(nil, at: MCPSettings.path + [service.id])
        let variables = Integrations.keys(of: service.way).map { IntegrationKeys.variable(service.id, $0.name) }
        // Bots here with keys of their own for it stop using it too: their keys go, and their
        // folders' settings stop naming its server.
        let bots = app.bots.list.filter { $0.machine == box.machine }
        Task {
            for bot in bots {
                let theirs = Set(Integrations.keys(of: service.way).map { IntegrationKeys.variable(service.id, $0.name, bot: bot.id) })
                let held = await Task.detached { IntegrationKeys.variables(ofBot: bot.id) }.value
                if !theirs.isDisjoint(with: held) { await BotKeys.remove(service, bot: bot, app: app) }
            }
        }
        guard let machine = box.machine else {
            for variable in variables { IntegrationKeys.set(nil, for: variable) }
            // What its server kept: its own folder (and the file it once kept).
            for name in [service.id, "\(service.id).json"] { try? FileManager.default.removeItem(at: AppInfo.supportDirectory.appending(path: "integrations/\(name)")) }
            return
        }
        // On the machine: its keys and what its server kept there, now or the next time it answers.
        let root = "\"$HOME/\(RemoteEngineInstaller.root)"
        let script = (["rm -rf \(root)/integrations/\(service.id)\""] + variables.map { "rm -f \(root)/box/keys/\($0)\"" }).joined(separator: "; ")
        let later = Machines.integrationMark(service.id) + script
        guard app.machines.engine(machine)?.isReady == true else { app.machines.runLater(later, on: machine); return }
        Task {
            let done = await Task.detached { RemoteShell.run(script, on: machine) != nil }.value
            if !done { app.machines.runLater(later, on: machine) }
        }
    }

    private func refresh() async {
        live = await MCPStatus.read(api) ?? []
    }
}

/// Where an integration runs, and how it's doing there.
private struct Place: Identifiable {
    let name: String
    /// nil for this Mac.
    let machine: String?
    let state: State
    /// Alone in its line, it still says where ("Signed in on this Mac").
    var namesPlace = false
    var id: String { machine ?? "" }

    /// Worst last.
    enum State: Int, Comparable {
        case connected, signedIn, starting, signedOut, waitsForCode, needsSignIn, failed

        static func < (a: State, b: State) -> Bool { a.rawValue < b.rawValue }

        var title: String {
            switch self {
            case .connected: "Connected"
            case .signedIn: "Signed in"
            case .signedOut: "Not signed in"
            case .starting: "Starting"
            case .waitsForCode: "Not signed in yet"
            case .needsSignIn: "Needs sign-in"
            case .failed: "Couldn’t connect"
            }
        }

        var color: Color {
            switch self {
            case .connected, .signedIn: .green
            case .starting, .signedOut: .secondary
            case .waitsForCode, .needsSignIn: .orange
            case .failed: .red
            }
        }
    }

    /// As an engine reports its server (none yet: it starts with the engine).
    static func state(_ server: MCPServer?) -> State {
        switch server?.status.status {
        case "connected": .connected
        case "needs_auth": .needsSignIn
        case "failed": .failed
        default: .starting
        }
    }
}

/// An integration's state in a line: a dot in the color of the worst, and each state with where
/// ("Connected on this Mac · Needs sign-in on my-server"); with no machines, just the state.
private struct PlacesLine: View {
    let places: [Place]
    let waitsForCodeHelp: String

    private var text: String {
        guard places.count > 1 else {
            guard let place = places.first else { return "" }
            return place.state.title + (place.namesPlace ? " on " + place.name : "")
        }
        let states = Set(places.map(\.state)).sorted()
        if states.count == 1, let state = states.first, [.connected, .signedIn].contains(state) { return state.title + " on " + Self.list(places.map(\.name)) }
        return states.reversed().map { state in
            state.title + " on " + Self.list(places.filter { $0.state == state }.map(\.name))
        }.joined(separator: " · ")
    }

    static func list(_ names: [String]) -> String {
        names.count <= 1 ? names.joined() : names.dropLast().joined(separator: ", ") + " and " + names.last!
    }

    var body: some View {
        let worst = places.map(\.state).max() ?? .connected
        HStack(spacing: 6) {
            Circle()
                .fill(worst.color)
                .frame(width: 7, height: 7)
            Text(text)
                .lineLimit(1)
        }
        .font(.subheadline)
        .foregroundStyle(.secondary)
        .help(places.contains { $0.state == .waitsForCode } ? waitsForCodeHelp : "")
    }
}

private struct KeyRequest: Identifiable {
    let service: Integrations.Service
    var id: String { service.id }
}

/// The keys an integration needs, typed or pasted, for this computer: this Mac's Keychain, or a
/// machine's own keys' folder: in the page's place.
private struct KeyEntry: View {
    let service: Integrations.Service
    let done: (Bool) -> Void
    @Environment(AppModel.self) private var app
    @Environment(\.settingsBox) private var box
    @State private var values: [String: String] = [:]
    @State private var failed: String?
    @State private var isSaving = false

    private var keys: [Integrations.Key] { Integrations.keys(of: service.way) }

    private var help: String? {
        switch service.way {
        case .officialToken(_, _, let help), .officialApp(_, let help): help
        case .package(_, _, let help, _, _, _): help
        default: nil
        }
    }

    /// What the keys are for, under the fields.
    private var explanation: String {
        switch service.way {
        case .officialApp:
            "\(service.title)'s own server signs in only through an app registered with it. Make one in its developer console, with this redirect URL, then paste its ID and secret: \(Integrations.redirect). They stay \(kept); then you sign in."
        case .officialToken:
            "It goes to \(service.title)'s own server with your token, which stays \(kept)\(box.machine == nil ? " and reaches the engine only while it runs" : "")."
        case .package(_, _, _, let by, _, _):
            "It runs \(by), an open-source MCP server, with your key. The key stays \(kept)\(box.machine == nil ? " and reaches the engine only while it runs" : "")."
        default:
            ""
        }
    }

    /// Where the keys are kept: in your Keychain, or in a folder only you can read on the machine.
    private var kept: String {
        box.machine == nil ? "in your Keychain" : "on \(app.name(of: box)), in a folder only you can read"
    }

    private var isComplete: Bool {
        keys.allSatisfy { !(values[$0.name] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    }

    var body: some View {
        SettingsDetailPage {
            Section {
                ForEach(keys, id: \.name) { key in
                    SettingsTextRow(title: key.label) {
                        SecureField(key.label, text: Binding(get: { values[key.name] ?? "" }, set: { values[key.name] = $0 }), prompt: Text(key.prompt))
                    }
                }
            } footer: {
                VStack(alignment: .leading, spacing: 6) {
                    if let failed {
                        Text(failed)
                            .foregroundStyle(.red)
                    }
                    Text(explanation)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    if let help, let url = URL(string: help) {
                        Link("Where to get one", destination: url)
                    }
                }
            }
        } footer: {
            EditorFooter(primary: "Save", canDoPrimary: isComplete && !isSaving, onPrimary: save, onCancel: { done(false) })
        }
        .onAppear {
            // This Mac's, to change; a machine's stay there.
            guard box.machine == nil else { return }
            for key in keys { values[key.name] = IntegrationKeys.value(IntegrationKeys.variable(service.id, key.name)) ?? "" }
        }
    }

    private func save() {
        let given = keys.map { (IntegrationKeys.variable(service.id, $0.name), values[$0.name]?.trimmingCharacters(in: .whitespacesAndNewlines)) }
        guard let machine = box.machine, let engine = app.machines.engine(machine) else {
            guard given.allSatisfy({ IntegrationKeys.set($0.1, for: $0.0) }) else {
                failed = "Couldn’t save it in your Keychain. Try again, or check Keychain Access."
                return
            }
            done(true)
            return
        }
        isSaving = true
        failed = nil
        Task {
            do {
                for (variable, value) in given { try await MachineBox.setKey(variable, value, on: engine.machine.connection) }
                done(true)
            } catch {
                failed = "Couldn’t save it on \(engine.machine.name): \(error.localizedDescription)"
            }
            isSaving = false
        }
    }
}
