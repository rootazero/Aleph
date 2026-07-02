import XCTest
@testable import AlephBridge

/// Unit tests for `locatorScore`, the pure scoring function backing
/// `ax.set_value` / `ax.perform_action` element resolution. Runs without
/// live AX handles.
final class AxLocatorScoreTests: XCTestCase {
    func testRoleMismatchRejects() {
        let result = locatorScore(
            locator: AxLocator(pid: nil, role: "AXButton", title: nil, center: nil),
            role: "AXTextField", title: nil, bounds: nil
        )
        XCTAssertNil(result)
    }

    func testExactTitleOutscoresContains() {
        let loc = AxLocator(pid: nil, role: nil, title: "Save", center: nil)
        let exact = locatorScore(locator: loc, role: "AXButton", title: "Save", bounds: nil)
        let contains = locatorScore(locator: loc, role: "AXButton", title: "Save As…", bounds: nil)
        XCTAssertNotNil(exact)
        XCTAssertNotNil(contains)
        XCTAssertGreaterThan(exact!, contains!)
    }

    func testNearestCenterWinsTiebreak() {
        let loc = AxLocator(pid: nil, role: "AXButton", title: nil, center: [100, 100])
        let near = locatorScore(
            locator: loc, role: "AXButton", title: nil,
            bounds: Region(x: 90, y: 90, width: 20, height: 20)
        )
        let far = locatorScore(
            locator: loc, role: "AXButton", title: nil,
            bounds: Region(x: 500, y: 500, width: 20, height: 20)
        )
        XCTAssertNotNil(near)
        XCTAssertNotNil(far)
        XCTAssertGreaterThan(near!, far!)
    }
}
