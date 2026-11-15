// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "OpenCodeKit",
    platforms: [.macOS(.v26), .iOS(.v26)],
    products: [
        .library(name: "OpenCodeKit", targets: ["OpenCodeKit"]),
    ],
    // Kuvo Helper's code moved to a package of its own (mac/helper). The retired Mac app still reads
    // it through OpenCodeKit (HelperKit.swift) until the box's release is built without that app
    // (#713): then this dependency goes. On the Mac only: the phone never builds it.
    dependencies: [.package(path: "../../mac/helper")],
    targets: [
        .target(name: "OpenCodeKit", dependencies: [.product(name: "KuvoHelperKit", package: "helper", condition: .when(platforms: [.macOS]))]),
        // A process for the tests to kill, as a crash would, with a server running under it.
        .executableTarget(name: "ServerProcessTestHost", dependencies: ["OpenCodeKit"], path: "Tests/ServerProcessTestHost"),
        .testTarget(name: "OpenCodeKitTests", dependencies: ["OpenCodeKit", "ServerProcessTestHost", .product(name: "KuvoHelperKit", package: "helper")], resources: [.copy("Fixtures")]),
    ]
)
