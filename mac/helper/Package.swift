// swift-tools-version: 6.2
import PackageDescription

// Kuvo Helper: the signed program this Mac's box runs under (protocol/PROTOCOL.md, "This Mac's
// box: its helper"), and `kuvo-keys`, the one-time hand-over of Kuvo Code 0.7's Keychain items to
// it. macOS only: a Linux box needs neither. Built into its bundle, on its own, by
// scripts/build-helper.sh.
let package = Package(
    name: "KuvoHelper",
    platforms: [.macOS(.v26)],
    products: [
        .library(name: "KuvoHelperKit", targets: ["KuvoHelperKit"]),
        .executable(name: "KuvoHelper", targets: ["KuvoHelper"]),
        .executable(name: "kuvo-keys", targets: ["KuvoKeys"]),
    ],
    targets: [
        .target(name: "KuvoHelperKit"),
        .executableTarget(name: "KuvoHelper", dependencies: ["KuvoHelperKit"]),
        .executableTarget(name: "KuvoKeys", dependencies: ["KuvoHelperKit"]),
        .testTarget(name: "KuvoHelperTests", dependencies: ["KuvoHelperKit", "KuvoHelper", "KuvoKeys"]),
    ]
)
