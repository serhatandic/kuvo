// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Terrarium",
    platforms: [.macOS(.v15)],
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser", from: "1.5.0"),
    ],
    targets: [
        .executableTarget(
            name: "Terrarium",
            dependencies: [.product(name: "ArgumentParser", package: "swift-argument-parser")],
            path: "Sources/Terrarium",
            swiftSettings: [.unsafeFlags(["-Osize"], .when(configuration: .release))]
        ),
    ]
)
