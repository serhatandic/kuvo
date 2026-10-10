// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Terrarium",
    platforms: [.macOS(.v15)],
    targets: [
        .executableTarget(
            name: "Terrarium",
            path: "Sources/Terrarium",
            swiftSettings: [.unsafeFlags(["-Osize"], .when(configuration: .release))]
        ),
    ]
)
