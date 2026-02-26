import XCTest
@testable import Aleph

final class AlephTests: XCTestCase {

    /// Verify that the app module can be imported and basic types are accessible.
    func testAppDelegateExists() throws {
        let delegate = AppDelegate()
        XCTAssertNotNil(delegate, "AppDelegate should be instantiable")
    }

    /// Verify that applicationShouldTerminateAfterLastWindowClosed returns false
    /// (menu bar app should stay alive when windows close).
    func testShouldNotTerminateAfterLastWindowClosed() throws {
        let delegate = AppDelegate()
        let result = delegate.applicationShouldTerminateAfterLastWindowClosed(NSApplication.shared)
        XCTAssertFalse(result, "Menu bar app should not terminate when last window closes")
    }
}
