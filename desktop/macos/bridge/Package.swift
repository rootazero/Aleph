// swift-tools-version: 5.9
import PackageDescription

// No external dependencies: the helper is a pure JSON-RPC server over
// stdin/stdout (see Sources/AlephBridge/main.swift).
let package = Package(
    name: "AlephBridge",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "AlephBridge",
            path: "Sources/AlephBridge"
        ),
        .testTarget(
            name: "AlephBridgeTests",
            dependencies: ["AlephBridge"],
            path: "Tests/AlephBridgeTests",
            resources: [.copy("Fixtures")]
        ),
    ]
)
