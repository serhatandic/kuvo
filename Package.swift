// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Kuvo",
    platforms: [.macOS(.v15)],
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser", from: "1.5.0"),
        // 1.11.x is the last line without Metal shaders, so building needs no extra Xcode components.
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", .upToNextMinor(from: "1.11.2")),
    ],
    targets: [
        .executableTarget(
            name: "Kuvo",
            dependencies: [
                .product(name: "ArgumentParser", package: "swift-argument-parser"),
                .product(name: "SwiftTerm", package: "SwiftTerm"),
            ],
            path: "Sources/Kuvo",
            swiftSettings: [.unsafeFlags(["-Osize"], .when(configuration: .release))]
        ),
    ]
)
