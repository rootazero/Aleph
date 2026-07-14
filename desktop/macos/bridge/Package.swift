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
        // Test-only GUI fixture for the computer-use e2e loop
        // (desktop/macos/tests/computer_use_e2e.rs). It is a target of this
        // package only because it needs AppKit and a Swift toolchain — nothing in
        // AlephBridge references it, and it is never bundled into a product: the
        // release build (`just swift-bridge`) asks for `--product AlephBridge`
        // explicitly, and only `just swift-fixture` builds this one.
        .executableTarget(
            name: "AlephFixture",
            path: "Sources/AlephFixture"
        ),
        .testTarget(
            name: "AlephBridgeTests",
            dependencies: ["AlephBridge"],
            path: "Tests/AlephBridgeTests",
            resources: [.copy("Fixtures")]
        ),
    ]
)
