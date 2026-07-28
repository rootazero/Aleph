import XCTest
@testable import AlephBridge

/// The node budget and its wire shape.
///
/// The helper used to stop at a private `MAX_TREE_NODES = 10_000` and say
/// nothing about it — a caller handed a clipped subtree could not tell it apart
/// from a small application. Windows stopped at 4 000 and Linux at 1 500, each
/// silently. The budget is the protocol's now; these tests pin this side of it.
final class AxBudgetTests: XCTestCase {

    // MARK: - Clamping (mirrors `ax::clamp_max_nodes` on the Rust side)

    func testAbsentBudgetFallsBackToTheProtocolDefault() {
        XCTAssertEqual(clampMaxNodes(nil), 1_500)
    }

    /// Zero means "unspecified", never "return nothing": an empty tree is the
    /// one answer a caller cannot tell apart from an inaccessible app.
    func testZeroAndNegativeAreTreatedAsUnspecified() {
        XCTAssertEqual(clampMaxNodes(0), 1_500)
        XCTAssertEqual(clampMaxNodes(-1), 1_500)
    }

    func testAnExplicitBudgetIsHonoured() {
        XCTAssertEqual(clampMaxNodes(1), 1)
        XCTAssertEqual(clampMaxNodes(700), 700)
    }

    /// A request cannot be talked into an unbounded walk — every node is several
    /// round trips into another process.
    func testAnOversizedBudgetIsCappedNotObeyed() {
        XCTAssertEqual(clampMaxNodes(10_001), 10_000)
        XCTAssertEqual(clampMaxNodes(Int.max), 10_000)
    }

    // MARK: - Wire shape

    /// A walk that did not run out reports so explicitly; the Rust decoder reads
    /// both fields and a missing one means "not told", never "no".
    func testQueryResultCarriesTheBudgetAccounting() throws {
        var budget = WalkBudget()
        budget.nodeCount = 42
        budget.truncated = true

        let json = try JSONEncoder().encode(QueryResult(element: nil, budget: budget))
        let decoded = try JSONSerialization.jsonObject(with: json) as? [String: Any]

        XCTAssertEqual(decoded?["node_count"] as? Int, 42)
        XCTAssertEqual(decoded?["truncated"] as? Bool, true)
    }

    func testAFreshBudgetReportsAnUntruncatedWalk() throws {
        let json = try JSONEncoder().encode(QueryListResult(elements: []))
        let decoded = try JSONSerialization.jsonObject(with: json) as? [String: Any]

        XCTAssertEqual(decoded?["node_count"] as? Int, 0)
        XCTAssertEqual(decoded?["truncated"] as? Bool, false)
    }

    // MARK: - Focused-element params

    /// An older client sends no params for `ax.query_focused` at all, which has
    /// to keep meaning the system-wide question it used to be the only way to
    /// ask.
    func testFocusedParamsDecodeWithoutAPid() throws {
        let params = try JSONDecoder().decode(
            QueryFocusedParams.self, from: Data("{}".utf8)
        )
        XCTAssertNil(params.pid)
    }

    func testFocusedParamsCarryAPidWhenGiven() throws {
        let params = try JSONDecoder().decode(
            QueryFocusedParams.self, from: Data(#"{"pid":733}"#.utf8)
        )
        XCTAssertEqual(params.pid, 733)
    }

    /// `max_nodes` is optional on the wire for the same reason.
    func testTreeParamsDecodeWithoutABudget() throws {
        let params = try JSONDecoder().decode(
            QueryTreeParams.self, from: Data(#"{"pid":1,"max_depth":6}"#.utf8)
        )
        XCTAssertNil(params.max_nodes)
        XCTAssertEqual(clampMaxNodes(params.max_nodes), 1_500)
    }
}
