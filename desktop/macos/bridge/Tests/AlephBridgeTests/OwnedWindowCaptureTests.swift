import XCTest
@testable import AlephBridge

final class OwnedWindowCaptureTests: XCTestCase {
    private final class SyntheticWindow {
        let windowID: CGWindowID
        let ownerPID: Int32?

        init(windowID: CGWindowID, ownerPID: Int32?) {
            self.windowID = windowID
            self.ownerPID = ownerPID
        }
    }

    private let validParams = #"{"window_id":900,"expected_pid":4242}"#

    func testStrictDecoderRejectsInvalidOperands() throws {
        let invalid = [
            #"{"expected_pid":4242}"#,
            #"{"window_id":900}"#,
            #"{"window_id":900.5,"expected_pid":4242}"#,
            #"{"window_id":900,"expected_pid":4242.5}"#,
            #"{"window_id":-1,"expected_pid":4242}"#,
            #"{"window_id":900,"expected_pid":-1}"#,
            #"{"window_id":0,"expected_pid":4242}"#,
            #"{"window_id":900,"expected_pid":0}"#,
            #"{"window_id":4294967296,"expected_pid":4242}"#,
            #"{"window_id":900,"expected_pid":2147483648}"#,
        ]

        for json in invalid {
            let params = try JSONDecoder().decode(JSONValue.self, from: Data(json.utf8))
            XCTAssertThrowsError(try decodeOwnedWindowCaptureParams(params), json) { error in
                XCTAssertEqual((error as? RpcError)?.code, -32602, json)
            }
        }
        XCTAssertThrowsError(try decodeOwnedWindowCaptureParams(nil))
    }

    func testOnlyExactWindowAndOwnerCanReachCapture() throws {
        let request = try decodeOwnedWindowCaptureParams(
            JSONDecoder().decode(JSONValue.self, from: Data(validParams.utf8))
        )
        XCTAssertEqual(request.windowId, 900)
        XCTAssertEqual(request.expectedPid, 4242)

        let matching = SyntheticWindow(windowID: 900, ownerPID: 4242)
        let rejected = [
            SyntheticWindow(windowID: 901, ownerPID: 4242),
            SyntheticWindow(windowID: 900, ownerPID: nil),
            SyntheticWindow(windowID: 900, ownerPID: 4243),
        ]
        var captureRequests: [(CGWindowID, Int32)] = []

        func attemptCapture(_ candidate: SyntheticWindow?) throws {
            let selected = try selectOwnedWindow(
                candidate,
                windowID: { $0.windowID },
                ownerPID: { $0.ownerPID },
                request: request
            )
            captureRequests.append((selected.windowID, request.expectedPid))
        }

        XCTAssertThrowsError(try attemptCapture(nil))
        for candidate in rejected {
            XCTAssertThrowsError(try attemptCapture(candidate))
        }
        XCTAssertTrue(captureRequests.isEmpty)

        try attemptCapture(matching)
        XCTAssertEqual(captureRequests.count, 1)
        XCTAssertEqual(captureRequests[0].0, 900)
        XCTAssertEqual(captureRequests[0].1, 4242)
    }
}
