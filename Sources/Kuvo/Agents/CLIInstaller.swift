import Foundation

/// The `kuvo` command ships inside the app (it's the same binary). On
/// launch we make sure it's on the PATH: a symlink in a folder that already
/// is, or else ~/.kuvo/bin plus one line in the shell profile.
enum CLIInstaller {
    /// The real binary, even when invoked through the `kuvo` symlink.
    static var executable: String {
        URL(filePath: Bundle.main.executablePath ?? CommandLine.arguments[0]).resolvingSymlinksInPath().path
    }

    /// The enclosing Kuvo.app, if running from one.
    static var appBundle: Bundle? {
        let app = URL(filePath: executable).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        return app.pathExtension == "app" ? Bundle(url: app) : nil
    }

    private static let fallback = ("~/.kuvo/bin" as NSString).expandingTildeInPath
    private static var folders: [String] { ["/opt/homebrew/bin", "/usr/local/bin", fallback] }

    /// Where `kuvo` points at this copy of the app, if anywhere.
    static var installedPath: String? {
        folders.map { "\($0)/kuvo" }.first { (try? FileManager.default.destinationOfSymbolicLink(atPath: $0)) == executable }
    }

    /// Runs on every launch of an installed app; cheap when already in place.
    @discardableResult
    static func ensureInstalled() -> String? {
        guard appBundle != nil else { return nil } // Never put a development build on the PATH.
        if let installedPath { return installedPath }
        let fm = FileManager.default
        for folder in folders.dropLast() where fm.isWritableFile(atPath: folder) {
            if let link = link(in: folder) { return link }
        }
        try? fm.createDirectory(atPath: fallback, withIntermediateDirectories: true)
        guard let link = link(in: fallback) else { return nil }
        addToShellProfiles()
        return link
    }

    /// Links `kuvo` in a folder, replacing only a stale link of ours.
    private static func link(in folder: String) -> String? {
        let fm = FileManager.default
        let link = "\(folder)/kuvo"
        if let destination = try? fm.destinationOfSymbolicLink(atPath: link) {
            guard destination.contains("Kuvo") else { return nil } // Someone else's.
            try? fm.removeItem(atPath: link)
        } else if fm.fileExists(atPath: link) {
            return nil // A real file we didn't put there.
        }
        return (try? fm.createSymbolicLink(atPath: link, withDestinationPath: executable)) != nil ? link : nil
    }

    private static func addToShellProfiles() {
        let line = "\n# Added by Kuvo Box: the kuvo command-line tool\nexport PATH=\"$HOME/.kuvo/bin:$PATH\"\n"
        let home = FileManager.default.homeDirectoryForCurrentUser
        var profiles = [home.appending(path: ".zprofile")] // zsh is the macOS default.
        let bash = home.appending(path: ".bash_profile")
        if FileManager.default.fileExists(atPath: bash.path) { profiles.append(bash) }
        for profile in profiles {
            let existing = (try? String(contentsOf: profile, encoding: .utf8)) ?? ""
            guard !existing.contains(".kuvo/bin") else { continue }
            try? (existing + line).write(to: profile, atomically: true, encoding: .utf8)
        }
    }
}
