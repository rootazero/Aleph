// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "AlephBridge",
    platforms: [.macOS(.v13)],
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.3.0"),
    ],
    targets: [
        .executableTarget(
            name: "AlephBridge",
            dependencies: [
                .product(name: "ArgumentParser", package: "swift-argument-parser"),
            ],
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
