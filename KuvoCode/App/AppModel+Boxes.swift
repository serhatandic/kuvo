import OpenCodeKit
import SwiftUI

/// Each computer's own, as a box of its own (this Mac, or a machine): its engine's settings, its
/// engine, its models and its coding agents, for what a page or a tab is about.
extension AppModel {
    /// Its engine's settings: this Mac's file, or the machine's own (read once it's reached).
    func config(for box: SidebarBox) -> EngineConfig {
        guard let machine = box.machine else { return engineConfig }
        return machines.engine(machine)?.config ?? Self.noSettings
    }

    /// Its engine, while it's running.
    func api(for box: SidebarBox) -> OpenCodeAPI? {
        guard let machine = box.machine else { return api }
        return machines.engine(machine)?.api
    }

    /// Its models, accounts and agents, as its engine lists them.
    func catalog(for box: SidebarBox) -> Catalog {
        guard let machine = box.machine else { return catalog }
        return machines.engine(machine)?.catalog ?? catalog
    }

    /// Its coding agents, beside its engine.
    func agents(for box: SidebarBox) -> OutsideAgents {
        guard let machine = box.machine else { return agents }
        return machines.engine(machine)?.agents ?? agents
    }

    /// What it's called: this Mac's name, or the machine's.
    func name(of box: SidebarBox) -> String {
        guard let machine = box.machine else { return PhoneLink.computerName }
        return machines.list.first { $0.id == machine }?.name ?? machine
    }

    /// This Mac, then your machines.
    var boxes: [SidebarBox] {
        [.mac] + machines.list.map { .machine($0.id) }
    }

    /// A machine that isn't added any more: nothing there to show or change.
    private static let noSettings = EngineConfig(draft: [:])
}

extension EnvironmentValues {
    /// The computer a page of Settings is about: this Mac, or a machine, with its own settings.
    @Entry var settingsBox: SidebarBox = .mac
}
