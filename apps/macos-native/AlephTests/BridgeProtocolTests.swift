import XCTest
@testable import Aleph

final class BridgeProtocolTests: XCTestCase {

    private let decoder = JSONDecoder()
    private let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.outputFormatting = [.sortedKeys]
        return e
    }()

    // MARK: - BridgeRequest Decoding

    func testDecodeRequestWithParams() throws {
        let json = """
        {
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "desktop.click",
            "params": { "x": 100.5, "y": 200.0 }
        }
        """.data(using: .utf8)!

        let request = try decoder.decode(BridgeRequest.self, from: json)
        XCTAssertEqual(request.jsonrpc, "2.0")
        XCTAssertEqual(request.id, "req-1")
        XCTAssertEqual(request.method, "desktop.click")
        XCTAssertNotNil(request.params)
        XCTAssertEqual(request.params?["x"]?.doubleValue, 100.5)
        XCTAssertEqual(request.params?["y"]?.doubleValue, 200.0)
    }

    func testDecodeRequestWithoutParams() throws {
        let json = """
        {
            "jsonrpc": "2.0",
            "id": "req-2",
            "method": "desktop.ping"
        }
        """.data(using: .utf8)!

        let request = try decoder.decode(BridgeRequest.self, from: json)
        XCTAssertEqual(request.jsonrpc, "2.0")
        XCTAssertEqual(request.id, "req-2")
        XCTAssertEqual(request.method, "desktop.ping")
        XCTAssertNil(request.params)
    }

    func testDecodeRequestWithNullParams() throws {
        let json = """
        {
            "jsonrpc": "2.0",
            "id": "req-3",
            "method": "desktop.screenshot",
            "params": null
        }
        """.data(using: .utf8)!

        let request = try decoder.decode(BridgeRequest.self, from: json)
        XCTAssertNil(request.params)
    }

    // MARK: - BridgeRequest Encoding

    func testEncodeRequestOmitsNilParams() throws {
        let request = BridgeRequest(id: "req-4", method: "desktop.ping")
        let data = try encoder.encode(request)
        let jsonString = String(data: data, encoding: .utf8)!

        // "params" key should be absent when nil
        XCTAssertFalse(jsonString.contains("params"))
        XCTAssertTrue(jsonString.contains("\"jsonrpc\":\"2.0\""))
        XCTAssertTrue(jsonString.contains("\"method\":\"desktop.ping\""))
    }

    func testEncodeRequestIncludesParams() throws {
        let request = BridgeRequest(
            id: "req-5",
            method: "desktop.type_text",
            params: ["text": AnyCodable("hello")]
        )
        let data = try encoder.encode(request)
        let jsonString = String(data: data, encoding: .utf8)!
        XCTAssertTrue(jsonString.contains("\"params\""))
        XCTAssertTrue(jsonString.contains("hello"))
    }

    // MARK: - BridgeResponse Encoding

    func testEncodeSuccessResponse() throws {
        let response = BridgeResponse.success(
            id: "resp-1",
            result: AnyCodable(["status": AnyCodable("ok")])
        )
        let data = try encoder.encode(response)
        let dict = try JSONSerialization.jsonObject(with: data) as! [String: Any]

        XCTAssertEqual(dict["jsonrpc"] as? String, "2.0")
        XCTAssertEqual(dict["id"] as? String, "resp-1")
        XCTAssertNotNil(dict["result"])
        XCTAssertNil(dict["error"])

        let result = dict["result"] as? [String: Any]
        XCTAssertEqual(result?["status"] as? String, "ok")
    }

    func testEncodeErrorResponse() throws {
        let response = BridgeResponse.error(
            id: "resp-2",
            error: BridgeRpcError(code: .methodNotFound, message: "Method not found")
        )
        let data = try encoder.encode(response)
        let dict = try JSONSerialization.jsonObject(with: data) as! [String: Any]

        XCTAssertEqual(dict["jsonrpc"] as? String, "2.0")
        XCTAssertEqual(dict["id"] as? String, "resp-2")
        XCTAssertNil(dict["result"])
        XCTAssertNotNil(dict["error"])

        let error = dict["error"] as? [String: Any]
        XCTAssertEqual(error?["code"] as? Int, -32601)
        XCTAssertEqual(error?["message"] as? String, "Method not found")
    }

    // MARK: - BridgeMethod Constants

    func testMethodConstants() {
        // Verify all method strings match the Rust constants
        XCTAssertEqual(BridgeMethod.ping.rawValue, "desktop.ping")
        XCTAssertEqual(BridgeMethod.screenshot.rawValue, "desktop.screenshot")
        XCTAssertEqual(BridgeMethod.ocr.rawValue, "desktop.ocr")
        XCTAssertEqual(BridgeMethod.axTree.rawValue, "desktop.ax_tree")
        XCTAssertEqual(BridgeMethod.click.rawValue, "desktop.click")
        XCTAssertEqual(BridgeMethod.typeText.rawValue, "desktop.type_text")
        XCTAssertEqual(BridgeMethod.keyCombo.rawValue, "desktop.key_combo")
        XCTAssertEqual(BridgeMethod.scroll.rawValue, "desktop.scroll")
        XCTAssertEqual(BridgeMethod.launchApp.rawValue, "desktop.launch_app")
        XCTAssertEqual(BridgeMethod.windowList.rawValue, "desktop.window_list")
        XCTAssertEqual(BridgeMethod.focusWindow.rawValue, "desktop.focus_window")
        XCTAssertEqual(BridgeMethod.canvasShow.rawValue, "desktop.canvas_show")
        XCTAssertEqual(BridgeMethod.canvasHide.rawValue, "desktop.canvas_hide")
        XCTAssertEqual(BridgeMethod.canvasUpdate.rawValue, "desktop.canvas_update")
        XCTAssertEqual(BridgeMethod.webviewShow.rawValue, "webview.show")
        XCTAssertEqual(BridgeMethod.webviewHide.rawValue, "webview.hide")
        XCTAssertEqual(BridgeMethod.webviewNavigate.rawValue, "webview.navigate")
        XCTAssertEqual(BridgeMethod.trayUpdateStatus.rawValue, "tray.update_status")
        XCTAssertEqual(BridgeMethod.bridgeShutdown.rawValue, "bridge.shutdown")
        XCTAssertEqual(BridgeMethod.handshake.rawValue, "aleph.handshake")
        XCTAssertEqual(BridgeMethod.systemPing.rawValue, "system.ping")
        XCTAssertEqual(BridgeMethod.capabilityRegister.rawValue, "capability.register")
    }

    // MARK: - BridgeErrorCode Constants

    func testErrorCodeConstants() {
        XCTAssertEqual(BridgeErrorCode.parse.rawValue, -32700)
        XCTAssertEqual(BridgeErrorCode.methodNotFound.rawValue, -32601)
        XCTAssertEqual(BridgeErrorCode.internal.rawValue, -32603)
        XCTAssertEqual(BridgeErrorCode.notImplemented.rawValue, -32000)
    }

    // MARK: - BridgeRpcError

    func testRpcErrorWithErrorCode() {
        let error = BridgeRpcError(code: .internal, message: "Something went wrong")
        XCTAssertEqual(error.code, -32603)
        XCTAssertEqual(error.message, "Something went wrong")
    }

    func testRpcErrorWithRawCode() {
        let error = BridgeRpcError(code: -32700, message: "Parse error")
        XCTAssertEqual(error.code, -32700)
    }

    // MARK: - AnyCodable Round-Trip

    func testAnyCodableBool() throws {
        let original: AnyCodable = true
        let data = try encoder.encode(original)
        let decoded = try decoder.decode(AnyCodable.self, from: data)
        XCTAssertEqual(decoded.boolValue, true)
        XCTAssertEqual(original, decoded)
    }

    func testAnyCodableInt() throws {
        let original: AnyCodable = 42
        let data = try encoder.encode(original)
        let decoded = try decoder.decode(AnyCodable.self, from: data)
        XCTAssertEqual(decoded.intValue, 42)
        XCTAssertEqual(original, decoded)
    }

    func testAnyCodableDouble() throws {
        let original: AnyCodable = 3.14
        let data = try encoder.encode(original)
        let decoded = try decoder.decode(AnyCodable.self, from: data)
        XCTAssertEqual(decoded.doubleValue, 3.14)
        XCTAssertEqual(original, decoded)
    }

    func testAnyCodableString() throws {
        let original: AnyCodable = "hello"
        let data = try encoder.encode(original)
        let decoded = try decoder.decode(AnyCodable.self, from: data)
        XCTAssertEqual(decoded.stringValue, "hello")
        XCTAssertEqual(original, decoded)
    }

    func testAnyCodableNull() throws {
        let original: AnyCodable = nil
        let data = try encoder.encode(original)
        let decoded = try decoder.decode(AnyCodable.self, from: data)
        XCTAssertTrue(decoded.isNull)
        XCTAssertEqual(original, decoded)
    }

    func testAnyCodableArray() throws {
        let original: AnyCodable = [1, "two", 3.0]
        let data = try encoder.encode(original)
        let decoded = try decoder.decode(AnyCodable.self, from: data)

        let arr = decoded.arrayValue
        XCTAssertNotNil(arr)
        XCTAssertEqual(arr?.count, 3)
        XCTAssertEqual(arr?[0].intValue, 1)
        XCTAssertEqual(arr?[1].stringValue, "two")
        XCTAssertEqual(arr?[2].doubleValue, 3.0)
    }

    func testAnyCodableDict() throws {
        let original: AnyCodable = ["key": "value", "count": 5]
        let data = try encoder.encode(original)
        let decoded = try decoder.decode(AnyCodable.self, from: data)

        let dict = decoded.dictValue
        XCTAssertNotNil(dict)
        XCTAssertEqual(dict?["key"]?.stringValue, "value")
        XCTAssertEqual(dict?["count"]?.intValue, 5)
    }

    func testAnyCodableNestedStructure() throws {
        let original: AnyCodable = [
            "name": "test",
            "metadata": [
                "tags": ["a", "b", "c"],
                "scores": [1, 2, 3],
                "nested": [
                    "deep": true
                ]
            ]
        ]

        let data = try encoder.encode(original)
        let decoded = try decoder.decode(AnyCodable.self, from: data)

        let dict = decoded.dictValue
        XCTAssertNotNil(dict)
        XCTAssertEqual(dict?["name"]?.stringValue, "test")

        let metadata = dict?["metadata"]?.dictValue
        XCTAssertNotNil(metadata)

        let tags = metadata?["tags"]?.arrayValue
        XCTAssertEqual(tags?.count, 3)
        XCTAssertEqual(tags?[0].stringValue, "a")
        XCTAssertEqual(tags?[1].stringValue, "b")
        XCTAssertEqual(tags?[2].stringValue, "c")

        let scores = metadata?["scores"]?.arrayValue
        XCTAssertEqual(scores?.count, 3)
        XCTAssertEqual(scores?[0].intValue, 1)

        let nested = metadata?["nested"]?.dictValue
        XCTAssertEqual(nested?["deep"]?.boolValue, true)
    }

    // MARK: - Wire Compatibility

    func testRequestRoundTrip() throws {
        let request = BridgeRequest(
            id: "uuid-1",
            method: BridgeMethod.screenshot.rawValue,
            params: [
                "region": AnyCodable([
                    "x": AnyCodable(0.0),
                    "y": AnyCodable(0.0),
                    "width": AnyCodable(1920.0),
                    "height": AnyCodable(1080.0)
                ])
            ]
        )

        let data = try encoder.encode(request)
        let decoded = try decoder.decode(BridgeRequest.self, from: data)

        XCTAssertEqual(decoded.jsonrpc, "2.0")
        XCTAssertEqual(decoded.id, "uuid-1")
        XCTAssertEqual(decoded.method, "desktop.screenshot")
        XCTAssertNotNil(decoded.params?["region"])

        let region = decoded.params?["region"]?.dictValue
        XCTAssertEqual(region?["width"]?.doubleValue, 1920.0)
        XCTAssertEqual(region?["height"]?.doubleValue, 1080.0)
    }

    // MARK: - Shared Value Types

    func testScreenRegionCodable() throws {
        let region = ScreenRegion(x: 10, y: 20, width: 100, height: 200)
        let data = try encoder.encode(region)
        let decoded = try decoder.decode(ScreenRegion.self, from: data)
        XCTAssertEqual(decoded, region)
    }

    func testCanvasPositionCodable() throws {
        let pos = CanvasPosition(x: 0, y: 0, width: 300, height: 400)
        let data = try encoder.encode(pos)
        let decoded = try decoder.decode(CanvasPosition.self, from: data)
        XCTAssertEqual(decoded, pos)
    }

    // MARK: - Capability Registration

    func testCapabilityRegistrationCodable() throws {
        let reg = CapabilityRegistration(
            platform: "macos",
            arch: "arm64",
            capabilities: [
                BridgeCapabilityInfo(name: "screenshot", version: "1.0"),
                BridgeCapabilityInfo(name: "ocr", version: "1.0"),
            ]
        )
        let data = try encoder.encode(reg)
        let decoded = try decoder.decode(CapabilityRegistration.self, from: data)
        XCTAssertEqual(decoded, reg)
        XCTAssertEqual(decoded.capabilities.count, 2)
        XCTAssertEqual(decoded.capabilities[0].name, "screenshot")
    }
}
