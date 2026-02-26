import XCTest
@testable import Aleph

final class ServerPathsTests: XCTestCase {
    func testAlephHomeDirectory() {
        let home = ServerPaths.alephHome
        XCTAssertTrue(home.path.hasSuffix(".aleph"))
    }

    func testBridgeSocketPath() {
        let socket = ServerPaths.bridgeSocket
        XCTAssertTrue(socket.path.hasSuffix("bridge.sock"))
        XCTAssertTrue(socket.path.contains(".aleph"))
    }

    func testServerBinaryPath() {
        // In test context, bundle won't contain server binary
        // Just verify the property is accessible (returns nil in test)
        _ = ServerPaths.serverBinary
    }

    func testConfigDirectory() {
        let config = ServerPaths.configDir
        XCTAssertTrue(config.path.contains("aleph"))
    }

    func testSettingsFilePath() {
        let settings = ServerPaths.settingsFile
        XCTAssertTrue(settings.path.hasSuffix("settings.json"))
    }
}
