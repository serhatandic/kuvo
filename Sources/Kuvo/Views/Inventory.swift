import SwiftUI

// Pieces shared by the sections that list things containers use: volumes and images.

/// "Unused  3 · 1.2 GB" with a button to remove them all.
struct CleanupHeader: View {
    let count: Int
    let size: Int64
    /// What's listed, for the button's tooltip and VoiceOver: "volumes", "images".
    let noun: String
    let removeAll: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            Text("Unused")
            Text("\(count) · \(Format.bytes(Double(size)))")
                .foregroundStyle(.tertiary)
                .monospacedDigit()
            Spacer(minLength: 4)
            Button("Remove All…", action: removeAll)
                .buttonStyle(.borderless)
                .font(.system(size: 11, weight: .medium))
                .help("Remove every unused \(noun.dropLast()) listed here")
        }
        .padding(.trailing, 12)
        // Sidebar section headers hide their contents from accessibility; offer the button as an action.
        .accessibilityElement(children: .combine)
        .accessibilityAction(named: "Remove All Unused \(noun.capitalized)", removeAll)
    }
}

/// "Used by web", "Used by web and db", "Used by web and 2 others", or, when
/// none of them runs, "Used by db, which is stopped": the reason Docker still
/// refuses to remove the thing.
func usedBy(_ users: [Container]) -> String {
    if !users.contains(where: \.isRunning) {
        return users.count == 1 ? "Used by \(users[0].name), which is stopped" : "Used by \(users.count) stopped containers"
    }
    return switch users.count {
    case 1: "Used by \(users[0].name)"
    case 2: "Used by \(users[0].name) and \(users[1].name)"
    default: "Used by \(users[0].name) and \(users.count - 1) others"
    }
}

/// Docker won't remove what a container uses, even a stopped one, so say what to do instead.
func removeBlockedHelp(_ users: [Container]) -> String {
    let names = users.map(\.name).formatted(.list(type: .and))
    let them = users.count == 1 ? "it" : "them"
    return users.contains(where: \.isRunning) ? "In use by \(names). Stop and remove \(them) first." : "Remove \(names) first."
}

/// A label and value in an Overview pane's grid.
struct DetailRow: View {
    let label: String
    let value: String
    var monospaced = false
    var help: String?

    init(_ label: String, _ value: String, monospaced: Bool = false, help: String? = nil) {
        self.label = label
        self.value = value
        self.monospaced = monospaced
        self.help = help
    }

    var body: some View {
        GridRow {
            Text(label)
                .foregroundStyle(.secondary)
                .gridColumnAlignment(.trailing)
            Text(value)
                .font(monospaced ? .system(size: 12).monospaced() : .system(size: 13))
                .textSelection(.enabled)
                .lineLimit(2)
                .truncationMode(.middle)
                .help(help ?? "")
        }
        .font(.system(size: 13))
    }
}

/// A container in a detail pane: its status, which decides whether the thing it
/// uses can be removed, and anything about how it uses it on the right.
/// Clicking it opens the container.
struct ContainerLinkRow<Trailing: View>: View {
    let container: Container
    /// Read by VoiceOver after the name and status, like "mounted at /data".
    var accessibilityDetail: String?
    let open: () -> Void
    @ViewBuilder var trailing: Trailing
    @State private var hovering = false

    var body: some View {
        Button(action: open) {
            HStack(spacing: 10) {
                StatusDot(state: container.state)
                VStack(alignment: .leading, spacing: 1) {
                    Text(container.name).font(.system(size: 13, weight: .medium))
                    Text(container.humanStatus)
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                .lineLimit(1)
                Spacer(minLength: 12)
                trailing
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.tertiary)
                    .opacity(hovering ? 1 : 0)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(.primary.opacity(hovering ? 0.06 : 0), in: .rect(cornerRadius: 8))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .accessibilityLabel([container.name, container.humanStatus, accessibilityDetail].compactMap(\.self).joined(separator: ", "))
        .accessibilityHint("Shows the container")
    }
}

extension ContainerLinkRow where Trailing == EmptyView {
    init(container: Container, open: @escaping () -> Void) {
        self.init(container: container, accessibilityDetail: nil, open: open) { EmptyView() }
    }
}

func copyToPasteboard(_ text: String) {
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(text, forType: .string)
}
