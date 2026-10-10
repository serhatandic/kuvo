import Foundation

/// Puts `terrarium` on the PATH as a symlink to the app's binary, in the
/// first folder the user can write to. Never replaces a real file.
enum CLIInstaller {
    /// The real binary, even when invoked through the `terrarium` symlink.
    static var executable: String {
        URL(filePath: Bundle.main.executablePath ?? CommandLine.arguments[0]).resolvingSymlinksInPath().path
    }

    /// The enclosing Terrarium.app, if running from one.
    static var appBundle: Bundle? {
        let app = URL(filePath: executable).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        return app.pathExtension == "app" ? Bundle(url: app) : nil
    }

    private static var folders: [String] {
        ["/opt/homebrew/bin", "/usr/local/bin", ("~/.local/bin" as NSString).expandingTildeInPath]
    }

    /// Where it's installed, if it points at this copy of the app.
    static var installedPath: String? {
        folders.map { "\($0)/terrarium" }.first { (try? FileManager.default.destinationOfSymbolicLink(atPath: $0)) == executable }
    }

    /// For when no folder on the PATH is writable.
    static var manualCommand: String { "sudo ln -sf '\(executable)' /usr/local/bin/terrarium" }

    static func install() throws -> String {
        let fm = FileManager.default
        for folder in folders where fm.isWritableFile(atPath: folder) {
            let link = "\(folder)/terrarium"
            if (try? fm.destinationOfSymbolicLink(atPath: link)) != nil {
                try fm.removeItem(atPath: link)
            } else if fm.fileExists(atPath: link) {
                continue // A real file we didn't put there.
            }
            try fm.createSymbolicLink(atPath: link, withDestinationPath: executable)
            return link
        }
        throw DockerError(message: "No writable folder on your PATH.")
    }
}
