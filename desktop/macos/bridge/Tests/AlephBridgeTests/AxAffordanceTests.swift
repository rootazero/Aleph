import XCTest
@testable import AlephBridge

/// Unit tests for the pure helpers behind the Chromium AX unlock and the
/// `AxElement` affordance fields, plus the wire shape the Rust decoder relies
/// on. All run without live AX handles.
final class AxAffordanceTests: XCTestCase {

    // MARK: - Secure-field detection

    func testSecureSubroleDetected() {
        XCTAssertTrue(isSecureSubrole("AXSecureTextField"))
    }

    /// A password field is role `AXTextField` + subrole `AXSecureTextField`, so
    /// nothing but the subrole can identify it.
    func testOrdinaryAndMissingSubrolesAreNotSecure() {
        XCTAssertFalse(isSecureSubrole("AXSearchField"))
        XCTAssertFalse(isSecureSubrole(nil))
    }

    // MARK: - Shell-tree detection (gates the AXEnhancedUserInterface escalation)

    func testAppWithNoChildrenIsAShell() {
        XCTAssertTrue(isShellTree(rootChildCount: 0, firstChildChildCount: nil))
    }

    func testLoneEmptyWindowIsAShell() {
        XCTAssertTrue(isShellTree(rootChildCount: 1, firstChildChildCount: 0))
    }

    /// The escalation's side effects are app-wide and sticky, so a healthy
    /// single-window app — one root child, but a populated window — must not
    /// trip it merely for having one window.
    func testHealthySingleWindowAppIsNotAShell() {
        XCTAssertFalse(isShellTree(rootChildCount: 1, firstChildChildCount: 12))
    }

    func testMultipleRootChildrenIsNotAShell() {
        XCTAssertFalse(isShellTree(rootChildCount: 3, firstChildChildCount: 0))
    }

    // MARK: - Wire shape

    /// Absent must stay absent: an affordance the limb could not read is omitted,
    /// never sent as `null`/`false`, so the Rust side reads "unknown" rather than
    /// "no" — and an element from a pre-affordance helper still decodes.
    func testUnknownAffordancesAreOmittedFromJson() throws {
        let el = AxElement(
            role: "AXGroup", title: nil, value: nil, bounds: nil, pid: 7,
            secure: nil, enabled: nil, settable: nil, actions: nil, url: nil,
            children: []
        )
        let data = try JSONEncoder().encode(el)
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(Set(obj.keys), ["role", "pid", "children"])
    }

    /// Field names and types are the contract with `AxElement` in
    /// `shared/protocol/src/desktop_bridge/methods/ax.rs`.
    func testKnownAffordancesUseTheAgreedNamesAndTypes() throws {
        let el = AxElement(
            role: "AXTextField", title: "Password", value: "hunter2", bounds: nil, pid: 7,
            secure: true, enabled: false, settable: true, actions: ["AXPress", "AXShowMenu"],
            url: "https://example.com/login",
            children: []
        )
        let data = try JSONEncoder().encode(el)
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["secure"] as? Bool, true)
        XCTAssertEqual(obj["enabled"] as? Bool, false)
        XCTAssertEqual(obj["settable"] as? Bool, true)
        XCTAssertEqual(obj["actions"] as? [String], ["AXPress", "AXShowMenu"])
        XCTAssertEqual(obj["url"] as? String, "https://example.com/login")
    }

    /// An empty action list is a fact ("this element does nothing"), not a gap,
    /// so it must survive as `[]` rather than collapsing to an omitted field.
    func testEmptyActionListSurvivesAsEmptyArray() throws {
        let el = AxElement(
            role: "AXStaticText", title: nil, value: "hello", bounds: nil, pid: 7,
            secure: false, enabled: nil, settable: false, actions: [], url: nil,
            children: []
        )
        let data = try JSONEncoder().encode(el)
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["actions"] as? [String], [])
        XCTAssertEqual(obj["secure"] as? Bool, false)
        XCTAssertNil(obj["enabled"])
    }
}
