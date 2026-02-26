import XCTest
@testable import Aleph

@MainActor
final class ServerManagerTests: XCTestCase {
    func testInitialStateIsStopped() {
        let manager = ServerManager()
        XCTAssertFalse(manager.isRunning)
        XCTAssertEqual(manager.state, .stopped)
    }

    func testStartWithMissingBinaryThrows() async {
        let manager = ServerManager()
        do {
            try await manager.start()
            XCTFail("Should throw when binary is missing")
        } catch ServerManager.Error.binaryNotFound {
            // Expected
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }

    func testSocketPathDefault() {
        let manager = ServerManager()
        XCTAssertEqual(manager.socketPath, ServerPaths.bridgeSocket)
    }

    func testSocketPathOverride() {
        let custom = URL(fileURLWithPath: "/tmp/test-aleph.sock")
        let manager = ServerManager(socketPath: custom)
        XCTAssertEqual(manager.socketPath, custom)
    }

    func testStateEquality() {
        XCTAssertEqual(ServerManager.State.stopped, ServerManager.State.stopped)
        XCTAssertEqual(ServerManager.State.running, ServerManager.State.running)
        XCTAssertNotEqual(ServerManager.State.stopped, ServerManager.State.running)
    }
}
