# Distribution Architecture Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement dual-track distribution: macOS native Xcode app + Tauri (Linux/Windows only) + pure server binary, all with embedded aleph-server.

**Architecture:** macOS uses Swift/SwiftUI with native system APIs (Vision, AX, CoreGraphics) for deep system integration. Linux/Windows continues with Tauri. All desktop apps embed aleph-server binary and manage its lifecycle via Process/NSTask. Communication uses existing UDS JSON-RPC 2.0 protocol.

**Tech Stack:** Swift 5.9+ / SwiftUI (macOS 13+), Xcode 15+, Foundation.Process, WKWebView, NWListener (Network.framework), Tauri 2, Cargo, GitHub Actions

**Reference:** Deprecated macOS app at `apps/macos/` contains 150+ Swift files with Vision OCR, AX API, CoreGraphics patterns. Bridge protocol defined in `shared/protocol/src/desktop_bridge.rs`.

---

## Phase 1: macOS Xcode Project Foundation

### Task 1: Create Xcode Project Scaffold

**Files:**
- Create: `apps/macos-native/Aleph.xcodeproj/` (via Xcode)
- Create: `apps/macos-native/Aleph/AlephApp.swift`
- Create: `apps/macos-native/Aleph/AppDelegate.swift`
- Create: `apps/macos-native/Aleph/Info.plist`
- Create: `apps/macos-native/Aleph/Aleph.entitlements`
- Create: `apps/macos-native/AlephTests/` (XCTest target)

> **Note:** Use `apps/macos-native/` to avoid conflict with deprecated `apps/macos/`. The old directory can be removed later.

**Step 1: Create Xcode project**

Open Xcode → File → New → Project → macOS → App:
- Product Name: `Aleph`
- Team: None (development signing)
- Organization Identifier: `com.aleph`
- Interface: SwiftUI
- Language: Swift
- Storage: None
- Include Tests: Yes
- Location: `apps/macos-native/`

**Step 2: Configure project settings**

In Xcode project settings:
- Deployment target: macOS 13.0
- App Category: `public.app-category.productivity`
- LSUIElement: YES (hide from Dock by default, menu bar app)
- Bundle identifier: `com.aleph.app`

**Step 3: Configure entitlements**

```xml
<!-- Aleph.entitlements -->
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.app-sandbox</key>
    <false/>
    <key>com.apple.security.automation.apple-events</key>
    <true/>
    <key>com.apple.security.temporary-exception.apple-events</key>
    <array>
        <string>com.apple.systemevents</string>
    </array>
</dict>
</plist>
```

> **Important:** Sandbox disabled because we need unrestricted access to:
> - Unix Domain Sockets (`~/.aleph/bridge.sock`)
> - Process spawning (aleph-server)
> - Accessibility API
> - Screen Recording
> - CoreGraphics events

**Step 4: Write minimal app entry point**

```swift
// Aleph/AlephApp.swift
import SwiftUI

@main
struct AlephApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        // Menu bar app — no default window
        Settings {
            EmptyView()
        }
    }
}
```

```swift
// Aleph/AppDelegate.swift
import Cocoa

class AppDelegate: NSObject, NSApplicationDelegate {

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Hide dock icon (menu bar only mode)
        NSApp.setActivationPolicy(.accessory)
        print("Aleph launched (menu bar mode)")
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        return false
    }

    func applicationWillTerminate(_ notification: Notification) {
        print("Aleph shutting down")
    }
}
```

**Step 5: Build and verify**

Run: Cmd+B in Xcode, or:
```bash
cd apps/macos-native && xcodebuild -scheme Aleph -configuration Debug build
```
Expected: Build succeeds, app launches as menu bar app (no dock icon, no window).

**Step 6: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: scaffold Xcode project with menu bar app"
```

---

### Task 2: Add ServerPaths Utility

**Files:**
- Create: `apps/macos-native/Aleph/Server/ServerPaths.swift`
- Test: `apps/macos-native/AlephTests/ServerPathsTests.swift`

**Step 1: Write the failing test**

```swift
// AlephTests/ServerPathsTests.swift
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
        let path = ServerPaths.serverBinary
        // Just verify it attempts to find in bundle
        XCTAssertNotNil(path)
    }

    func testConfigDirectory() {
        let config = ServerPaths.configDir
        XCTAssertTrue(config.path.contains("aleph"))
    }
}
```

**Step 2: Run test to verify it fails**

Run: Cmd+U in Xcode, or:
```bash
cd apps/macos-native && xcodebuild test -scheme Aleph -destination 'platform=macOS'
```
Expected: FAIL — `ServerPaths` type not found.

**Step 3: Write implementation**

```swift
// Aleph/Server/ServerPaths.swift
import Foundation

/// Central path definitions for Aleph filesystem layout.
/// Mirrors the conventions used by aleph-server and Tauri bridge.
enum ServerPaths {

    /// ~/.aleph/
    static var alephHome: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".aleph")
    }

    /// ~/.aleph/bridge.sock — UDS socket for server ↔ bridge communication
    static var bridgeSocket: URL {
        alephHome.appendingPathComponent("bridge.sock")
    }

    /// Path to aleph-server binary embedded in app bundle
    static var serverBinary: URL? {
        Bundle.main.url(forResource: "aleph-server", withExtension: nil)
    }

    /// ~/.config/aleph/ — settings storage (matches Tauri convention)
    static var configDir: URL {
        if let config = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first {
            return config.appendingPathComponent("aleph")
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aleph")
    }

    /// ~/.config/aleph/settings.json
    static var settingsFile: URL {
        configDir.appendingPathComponent("settings.json")
    }

    /// Ensure required directories exist
    static func ensureDirectories() throws {
        let fm = FileManager.default
        try fm.createDirectory(at: alephHome, withIntermediateDirectories: true)
        try fm.createDirectory(at: configDir, withIntermediateDirectories: true)
    }
}
```

**Step 4: Run tests and verify they pass**

Run: Cmd+U in Xcode
Expected: All 4 tests PASS.

**Step 5: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add ServerPaths utility with tests"
```

---

## Phase 2: Server Lifecycle Management

### Task 3: Implement ServerManager

**Files:**
- Create: `apps/macos-native/Aleph/Server/ServerManager.swift`
- Test: `apps/macos-native/AlephTests/ServerManagerTests.swift`

**Step 1: Write the failing test**

```swift
// AlephTests/ServerManagerTests.swift
import XCTest
@testable import Aleph

final class ServerManagerTests: XCTestCase {

    func testInitialStateIsNotRunning() {
        let manager = ServerManager()
        XCTAssertFalse(manager.isRunning)
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
        let custom = URL(fileURLWithPath: "/tmp/test.sock")
        let manager = ServerManager(socketPath: custom)
        XCTAssertEqual(manager.socketPath, custom)
    }
}
```

**Step 2: Run tests to verify they fail**

Expected: FAIL — `ServerManager` not found.

**Step 3: Write implementation**

```swift
// Aleph/Server/ServerManager.swift
import Foundation
import os

/// Manages the lifecycle of the embedded aleph-server process.
///
/// Responsibilities:
/// - Locate server binary in app bundle
/// - Start/stop server process
/// - Monitor health and auto-restart on crash
/// - Detect and reuse existing server instances
@MainActor
final class ServerManager: ObservableObject {

    enum Error: Swift.Error {
        case binaryNotFound
        case alreadyRunning
        case startFailed(String)
        case socketTimeout
    }

    enum State: Equatable {
        case stopped
        case starting
        case running
        case stopping
        case crashed(String)
    }

    @Published private(set) var state: State = .stopped
    let socketPath: URL
    private var process: Process?
    private var stdoutPipe: Pipe?
    private var stderrPipe: Pipe?
    private let logger = Logger(subsystem: "com.aleph.app", category: "ServerManager")

    var isRunning: Bool { state == .running }

    init(socketPath: URL? = nil) {
        self.socketPath = socketPath ?? ServerPaths.bridgeSocket
    }

    /// Start aleph-server, or reuse existing instance if one is running.
    func start() async throws {
        guard state == .stopped || state == .crashed("") == false else {
            if state == .running { throw Error.alreadyRunning }
            // Allow restart from crashed state
            break
        }

        // Check for existing server on socket
        if await checkExistingServer() {
            logger.info("Reusing existing aleph-server instance")
            state = .running
            return
        }

        // Locate binary in bundle
        guard let binaryPath = ServerPaths.serverBinary else {
            throw Error.binaryNotFound
        }

        state = .starting
        logger.info("Starting aleph-server from \(binaryPath.path)")

        // Ensure directories exist
        try ServerPaths.ensureDirectories()

        // Clean stale socket
        try? FileManager.default.removeItem(at: socketPath)

        // Configure process
        let proc = Process()
        proc.executableURL = binaryPath
        proc.arguments = [
            "--bridge-mode",
            "--socket", socketPath.path
        ]

        // Capture output for logging
        let stdout = Pipe()
        let stderr = Pipe()
        proc.standardOutput = stdout
        proc.standardError = stderr
        self.stdoutPipe = stdout
        self.stderrPipe = stderr

        // Monitor termination
        proc.terminationHandler = { [weak self] proc in
            Task { @MainActor in
                guard let self = self else { return }
                if self.state == .stopping {
                    self.state = .stopped
                } else {
                    let reason = "Exit code: \(proc.terminationStatus)"
                    self.logger.error("Server crashed: \(reason)")
                    self.state = .crashed(reason)
                }
            }
        }

        do {
            try proc.run()
            self.process = proc
        } catch {
            state = .stopped
            throw Error.startFailed(error.localizedDescription)
        }

        // Wait for socket to become available
        try await waitForSocket(timeout: 10.0)
        state = .running
        logger.info("aleph-server is ready")
    }

    /// Gracefully stop the server (SIGTERM → 5s wait → SIGKILL).
    func stop() async {
        guard let proc = process, proc.isRunning else {
            state = .stopped
            return
        }

        state = .stopping
        logger.info("Stopping aleph-server (PID: \(proc.processIdentifier))")

        // Send SIGTERM
        proc.terminate()

        // Wait up to 5 seconds for graceful shutdown
        let deadline = Date().addingTimeInterval(5.0)
        while proc.isRunning && Date() < deadline {
            try? await Task.sleep(nanoseconds: 100_000_000) // 100ms
        }

        // Force kill if still running
        if proc.isRunning {
            logger.warning("Force killing server")
            kill(proc.processIdentifier, SIGKILL)
        }

        proc.waitUntilExit()
        self.process = nil
        state = .stopped

        // Clean up socket
        try? FileManager.default.removeItem(at: socketPath)
        logger.info("aleph-server stopped")
    }

    // MARK: - Private

    /// Check if an existing server is listening on the socket.
    private func checkExistingServer() async -> Bool {
        guard FileManager.default.fileExists(atPath: socketPath.path) else {
            return false
        }
        // Try connecting to socket
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return false }
        defer { close(fd) }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = socketPath.path.utf8CString
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            pathBytes.withUnsafeBufferPointer { buf in
                let raw = UnsafeMutableRawPointer(ptr)
                raw.copyMemory(from: buf.baseAddress!, byteCount: min(buf.count, 104))
            }
        }
        let result = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                connect(fd, sockPtr, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        return result == 0
    }

    /// Poll until socket file appears and is connectable.
    private func waitForSocket(timeout: TimeInterval) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if await checkExistingServer() {
                return
            }
            try await Task.sleep(nanoseconds: 200_000_000) // 200ms
        }
        throw Error.socketTimeout
    }
}
```

**Step 4: Run tests and verify they pass**

Expected: All 4 tests PASS.

**Step 5: Wire ServerManager into AppDelegate**

```swift
// Update AppDelegate.swift
import Cocoa

class AppDelegate: NSObject, NSApplicationDelegate {
    let serverManager = ServerManager()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)

        Task {
            do {
                try await serverManager.start()
            } catch {
                print("Failed to start server: \(error)")
                // Continue without server — still show menu bar
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        return false
    }

    func applicationWillTerminate(_ notification: Notification) {
        Task {
            await serverManager.stop()
        }
    }
}
```

**Step 6: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add ServerManager with lifecycle management"
```

---

## Phase 3: Bridge Server (UDS JSON-RPC 2.0)

### Task 4: Implement BridgeProtocol Models

**Files:**
- Create: `apps/macos-native/Aleph/Bridge/BridgeProtocol.swift`
- Test: `apps/macos-native/AlephTests/BridgeProtocolTests.swift`

> **Reference:** Must match `shared/protocol/src/desktop_bridge.rs` exactly.

**Step 1: Write the failing test**

```swift
// AlephTests/BridgeProtocolTests.swift
import XCTest
@testable import Aleph

final class BridgeProtocolTests: XCTestCase {

    func testDecodeRequest() throws {
        let json = """
        {"jsonrpc":"2.0","id":"1","method":"desktop.ping","params":{}}
        """
        let request = try JSONDecoder().decode(BridgeRequest.self, from: json.data(using: .utf8)!)
        XCTAssertEqual(request.jsonrpc, "2.0")
        XCTAssertEqual(request.id, "1")
        XCTAssertEqual(request.method, "desktop.ping")
    }

    func testDecodeRequestWithoutParams() throws {
        let json = """
        {"jsonrpc":"2.0","id":"2","method":"system.ping"}
        """
        let request = try JSONDecoder().decode(BridgeRequest.self, from: json.data(using: .utf8)!)
        XCTAssertNil(request.params)
    }

    func testEncodeSuccessResponse() throws {
        let response = BridgeResponse.success(id: "1", result: ["pong": true])
        let data = try JSONEncoder().encode(response)
        let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(json["jsonrpc"] as? String, "2.0")
        XCTAssertEqual(json["id"] as? String, "1")
        XCTAssertNotNil(json["result"])
    }

    func testEncodeErrorResponse() throws {
        let response = BridgeResponse.error(id: "1", code: -32601, message: "Method not found")
        let data = try JSONEncoder().encode(response)
        let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        let error = json["error"] as? [String: Any]
        XCTAssertEqual(error?["code"] as? Int, -32601)
        XCTAssertEqual(error?["message"] as? String, "Method not found")
    }

    func testMethodConstants() {
        XCTAssertEqual(BridgeMethod.ping, "desktop.ping")
        XCTAssertEqual(BridgeMethod.screenshot, "desktop.screenshot")
        XCTAssertEqual(BridgeMethod.ocr, "desktop.ocr")
        XCTAssertEqual(BridgeMethod.handshake, "aleph.handshake")
    }
}
```

**Step 2: Run tests to verify they fail**

Expected: FAIL — types not found.

**Step 3: Write implementation**

```swift
// Aleph/Bridge/BridgeProtocol.swift
import Foundation

// MARK: - JSON-RPC 2.0 Request

/// JSON-RPC 2.0 request from aleph-server.
/// Must match `BridgeRequest` in shared/protocol/src/desktop_bridge.rs
struct BridgeRequest: Codable {
    let jsonrpc: String
    let id: String
    let method: String
    let params: [String: AnyCodable]?
}

// MARK: - JSON-RPC 2.0 Response

/// JSON-RPC 2.0 response to aleph-server.
enum BridgeResponse {
    case success(id: String, result: Any)
    case error(id: String, code: Int, message: String)
}

extension BridgeResponse: Encodable {
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode("2.0", forKey: .jsonrpc)

        switch self {
        case .success(let id, let result):
            try container.encode(id, forKey: .id)
            // Wrap result as JSON
            let jsonData = try JSONSerialization.data(withJSONObject: result)
            let jsonValue = try JSONDecoder().decode(AnyCodable.self, from: jsonData)
            try container.encode(jsonValue, forKey: .result)

        case .error(let id, let code, let message):
            try container.encode(id, forKey: .id)
            try container.encode(BridgeRpcError(code: code, message: message), forKey: .error)
        }
    }

    private enum CodingKeys: String, CodingKey {
        case jsonrpc, id, result, error
    }
}

struct BridgeRpcError: Codable {
    let code: Int
    let message: String
}

// MARK: - Method Constants

/// Desktop Bridge method names.
/// Must match constants in shared/protocol/src/desktop_bridge.rs
enum BridgeMethod {
    static let ping = "desktop.ping"
    static let screenshot = "desktop.screenshot"
    static let ocr = "desktop.ocr"
    static let axTree = "desktop.ax_tree"
    static let click = "desktop.click"
    static let typeText = "desktop.type_text"
    static let keyCombo = "desktop.key_combo"
    static let scroll = "desktop.scroll"
    static let launchApp = "desktop.launch_app"
    static let windowList = "desktop.window_list"
    static let focusWindow = "desktop.focus_window"
    static let canvasShow = "desktop.canvas_show"
    static let canvasHide = "desktop.canvas_hide"
    static let canvasUpdate = "desktop.canvas_update"
    static let webviewShow = "webview.show"
    static let webviewHide = "webview.hide"
    static let webviewNavigate = "webview.navigate"
    static let trayUpdateStatus = "tray.update_status"
    static let bridgeShutdown = "bridge.shutdown"
    static let handshake = "aleph.handshake"
    static let systemPing = "system.ping"
}

// MARK: - Error Codes

enum BridgeErrorCode {
    static let parseError = -32700
    static let methodNotFound = -32601
    static let internalError = -32603
    static let notImplemented = -32000
}

// MARK: - AnyCodable (type-erased JSON value)

/// Type-erased Codable wrapper for arbitrary JSON values.
struct AnyCodable: Codable {
    let value: Any

    init(_ value: Any) {
        self.value = value
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            value = NSNull()
        } else if let bool = try? container.decode(Bool.self) {
            value = bool
        } else if let int = try? container.decode(Int.self) {
            value = int
        } else if let double = try? container.decode(Double.self) {
            value = double
        } else if let string = try? container.decode(String.self) {
            value = string
        } else if let array = try? container.decode([AnyCodable].self) {
            value = array.map(\.value)
        } else if let dict = try? container.decode([String: AnyCodable].self) {
            value = dict.mapValues(\.value)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Unsupported type")
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch value {
        case is NSNull:
            try container.encodeNil()
        case let bool as Bool:
            try container.encode(bool)
        case let int as Int:
            try container.encode(int)
        case let double as Double:
            try container.encode(double)
        case let string as String:
            try container.encode(string)
        case let array as [Any]:
            try container.encode(array.map { AnyCodable($0) })
        case let dict as [String: Any]:
            try container.encode(dict.mapValues { AnyCodable($0) })
        default:
            try container.encodeNil()
        }
    }
}
```

**Step 4: Run tests and verify they pass**

Expected: All 5 tests PASS.

**Step 5: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add BridgeProtocol JSON-RPC 2.0 models"
```

---

### Task 5: Implement BridgeServer (UDS Listener)

**Files:**
- Create: `apps/macos-native/Aleph/Bridge/BridgeServer.swift`
- Test: `apps/macos-native/AlephTests/BridgeServerTests.swift`

> **Reference:** Must match behavior of `apps/desktop/src-tauri/src/bridge/mod.rs`

**Step 1: Write the failing test**

```swift
// AlephTests/BridgeServerTests.swift
import XCTest
@testable import Aleph

final class BridgeServerTests: XCTestCase {

    func testServerStartsAndStops() async throws {
        let socketPath = FileManager.default.temporaryDirectory
            .appendingPathComponent("test-\(UUID().uuidString).sock")
        let server = BridgeServer(socketPath: socketPath)

        try await server.start()
        XCTAssertTrue(server.isListening)

        // Verify socket file exists
        XCTAssertTrue(FileManager.default.fileExists(atPath: socketPath.path))

        await server.stop()
        XCTAssertFalse(server.isListening)
    }

    func testHandshakeResponse() async throws {
        let socketPath = FileManager.default.temporaryDirectory
            .appendingPathComponent("test-\(UUID().uuidString).sock")
        let server = BridgeServer(socketPath: socketPath)
        try await server.start()
        defer { Task { await server.stop() } }

        // Send handshake request via UDS
        let response = try await sendRequest(
            to: socketPath,
            method: "aleph.handshake",
            params: ["protocol_version": "1.0"]
        )

        // Verify response structure
        let result = response["result"] as? [String: Any]
        XCTAssertNotNil(result)
        XCTAssertEqual(result?["platform"] as? String, "macOS")
        XCTAssertNotNil(result?["capabilities"])
    }

    func testPingPong() async throws {
        let socketPath = FileManager.default.temporaryDirectory
            .appendingPathComponent("test-\(UUID().uuidString).sock")
        let server = BridgeServer(socketPath: socketPath)
        try await server.start()
        defer { Task { await server.stop() } }

        let response = try await sendRequest(
            to: socketPath,
            method: "desktop.ping",
            params: [:]
        )
        let result = response["result"] as? String
        XCTAssertEqual(result, "pong")
    }

    func testMethodNotFound() async throws {
        let socketPath = FileManager.default.temporaryDirectory
            .appendingPathComponent("test-\(UUID().uuidString).sock")
        let server = BridgeServer(socketPath: socketPath)
        try await server.start()
        defer { Task { await server.stop() } }

        let response = try await sendRequest(
            to: socketPath,
            method: "nonexistent.method",
            params: [:]
        )
        let error = response["error"] as? [String: Any]
        XCTAssertEqual(error?["code"] as? Int, -32601)
    }

    // MARK: - Helper

    private func sendRequest(
        to socketPath: URL,
        method: String,
        params: [String: Any]
    ) async throws -> [String: Any] {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw NSError(domain: "test", code: 1) }
        defer { close(fd) }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = socketPath.path.utf8CString
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            pathBytes.withUnsafeBufferPointer { buf in
                UnsafeMutableRawPointer(ptr).copyMemory(from: buf.baseAddress!, byteCount: min(buf.count, 104))
            }
        }
        let connectResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                connect(fd, sockPtr, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connectResult == 0 else { throw NSError(domain: "test", code: 2) }

        let request: [String: Any] = [
            "jsonrpc": "2.0",
            "id": UUID().uuidString,
            "method": method,
            "params": params
        ]
        var data = try JSONSerialization.data(withJSONObject: request)
        data.append(contentsOf: "\n".utf8)
        _ = data.withUnsafeBytes { write(fd, $0.baseAddress!, data.count) }

        // Read response
        var buffer = Data(count: 65536)
        let bytesRead = buffer.withUnsafeMutableBytes { read(fd, $0.baseAddress!, 65536) }
        guard bytesRead > 0 else { throw NSError(domain: "test", code: 3) }

        let responseData = buffer.prefix(bytesRead)
        return try JSONSerialization.jsonObject(with: responseData) as! [String: Any]
    }
}
```

**Step 2: Run tests to verify they fail**

Expected: FAIL — `BridgeServer` not found.

**Step 3: Write implementation**

```swift
// Aleph/Bridge/BridgeServer.swift
import Foundation
import Network
import os

/// UDS JSON-RPC 2.0 server for Desktop Bridge.
/// Symmetric with Tauri's `bridge::start_bridge_server()`.
///
/// Protocol: One JSON-RPC request per connection, newline-delimited.
/// Socket: ~/.aleph/bridge.sock (or override via init parameter).
actor BridgeServer {

    private let socketPath: URL
    private var listener: NWListener?
    private(set) var isListening = false
    private var handlers: [String: (([String: Any]) -> Result<Any, BridgeHandlerError>)] = [:]
    private let logger = Logger(subsystem: "com.aleph.app", category: "BridgeServer")

    struct BridgeHandlerError: Error {
        let code: Int
        let message: String
    }

    init(socketPath: URL? = nil) {
        self.socketPath = socketPath ?? ServerPaths.bridgeSocket
        registerDefaultHandlers()
    }

    /// Register a method handler
    func register(method: String, handler: @escaping ([String: Any]) -> Result<Any, BridgeHandlerError>) {
        handlers[method] = handler
    }

    /// Start listening on the UDS socket
    func start() throws {
        guard !isListening else { return }

        // Remove stale socket
        try? FileManager.default.removeItem(at: socketPath)

        // Ensure parent directory exists
        if let parent = socketPath.deletingLastPathComponent() as URL? {
            try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: true)
        }

        // Create NWListener on Unix Domain Socket
        let params = NWParameters()
        params.defaultProtocolStack.transportProtocol = NWProtocolTCP.Options()
        params.requiredLocalEndpoint = NWEndpoint.unix(path: socketPath.path)

        let listener = try NWListener(using: params)

        listener.stateUpdateHandler = { [weak self] state in
            switch state {
            case .ready:
                Task { await self?.setListening(true) }
            case .failed(let error):
                Task { await self?.handleListenerError(error) }
            default:
                break
            }
        }

        listener.newConnectionHandler = { [weak self] connection in
            Task { await self?.handleConnection(connection) }
        }

        listener.start(queue: .global(qos: .userInitiated))
        self.listener = listener

        // Set socket permissions (owner only)
        try? FileManager.default.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: socketPath.path
        )

        // Wait briefly for listener to be ready
        try await Task.sleep(nanoseconds: 100_000_000) // 100ms
        logger.info("BridgeServer listening on \(self.socketPath.path)")
    }

    /// Stop the server
    func stop() {
        listener?.cancel()
        listener = nil
        isListening = false
        try? FileManager.default.removeItem(at: socketPath)
        logger.info("BridgeServer stopped")
    }

    // MARK: - Private

    private func setListening(_ value: Bool) {
        isListening = value
    }

    private func handleListenerError(_ error: NWError) {
        logger.error("Listener error: \(error.localizedDescription)")
        isListening = false
    }

    private func handleConnection(_ connection: NWConnection) {
        connection.start(queue: .global(qos: .userInitiated))

        // Read one line (request), dispatch, write response, close
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, _, _, error in
            guard let self = self, let data = data, !data.isEmpty else {
                connection.cancel()
                return
            }

            Task {
                let response = await self.processRequest(data)
                var responseData = response
                responseData.append(contentsOf: "\n".utf8)
                connection.send(content: responseData, completion: .contentProcessed { _ in
                    connection.cancel()
                })
            }
        }
    }

    private func processRequest(_ data: Data) -> Data {
        // Parse JSON-RPC request
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let method = json["method"] as? String,
              let id = json["id"] as? String else {
            let error: [String: Any] = [
                "jsonrpc": "2.0",
                "id": NSNull(),
                "error": ["code": BridgeErrorCode.parseError, "message": "Parse error"]
            ]
            return (try? JSONSerialization.data(withJSONObject: error)) ?? Data()
        }

        let params = json["params"] as? [String: Any] ?? [:]

        // Dispatch to handler
        let result: [String: Any]
        if let handler = handlers[method] {
            switch handler(params) {
            case .success(let value):
                result = ["jsonrpc": "2.0", "id": id, "result": value]
            case .failure(let error):
                result = [
                    "jsonrpc": "2.0", "id": id,
                    "error": ["code": error.code, "message": error.message]
                ]
            }
        } else {
            result = [
                "jsonrpc": "2.0", "id": id,
                "error": ["code": BridgeErrorCode.methodNotFound, "message": "Method not found: \(method)"]
            ]
        }

        return (try? JSONSerialization.data(withJSONObject: result)) ?? Data()
    }

    // MARK: - Default Handlers

    private func registerDefaultHandlers() {
        // Ping
        handlers[BridgeMethod.ping] = { _ in .success("pong") }
        handlers[BridgeMethod.systemPing] = { _ in .success(["pong": true]) }

        // Handshake
        handlers[BridgeMethod.handshake] = { params in
            let protocolVersion = params["protocol_version"] as? String ?? "1.0"
            return .success([
                "protocol_version": protocolVersion,
                "bridge_type": "desktop",
                "platform": "macOS",
                "arch": Self.currentArch,
                "capabilities": [
                    ["name": "screen_capture", "version": "1.0"],
                    ["name": "webview", "version": "1.0"],
                    ["name": "tray", "version": "1.0"],
                    ["name": "global_hotkey", "version": "1.0"],
                    ["name": "notification", "version": "1.0"],
                    ["name": "keyboard_control", "version": "1.0"],
                    ["name": "mouse_control", "version": "1.0"],
                    ["name": "scroll", "version": "1.0"],
                    ["name": "canvas", "version": "1.0"],
                    ["name": "launch_app", "version": "1.0"],
                    ["name": "window_list", "version": "1.0"],
                    ["name": "focus_window", "version": "1.0"],
                    ["name": "ocr", "version": "1.0"],
                    ["name": "ax_inspect", "version": "1.0"],
                ] as [[String: String]]
            ] as [String: Any])
        }
    }

    private static var currentArch: String {
        #if arch(arm64)
        return "aarch64"
        #elseif arch(x86_64)
        return "x86_64"
        #else
        return "unknown"
        #endif
    }
}
```

**Step 4: Run tests and verify they pass**

Expected: All 4 tests PASS.

**Step 5: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add BridgeServer UDS JSON-RPC 2.0 listener"
```

---

## Phase 4: Desktop Capabilities

### Task 6: Implement ScreenCapture

**Files:**
- Create: `apps/macos-native/Aleph/Desktop/ScreenCapture.swift`

> **Reference:** `apps/desktop/src-tauri/src/bridge/perception.rs:41-91`

**Step 1: Write implementation**

```swift
// Aleph/Desktop/ScreenCapture.swift
import CoreGraphics
import Foundation

/// Screen capture using CoreGraphics (no third-party dependencies).
/// Replaces xcap crate used in Tauri bridge.
enum ScreenCapture {

    struct CaptureResult {
        let base64PNG: String
        let width: Int
        let height: Int
    }

    /// Check if screen recording permission is granted
    static var hasPermission: Bool {
        CGPreflightScreenCaptureAccess()
    }

    /// Request screen recording permission (shows system dialog)
    static func requestPermission() -> Bool {
        CGRequestScreenCaptureAccess()
    }

    /// Capture full screen or region, returns base64-encoded PNG.
    static func capture(region: CGRect? = nil) -> Result<CaptureResult, BridgeServer.BridgeHandlerError> {
        guard hasPermission else {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Screen Recording permission not granted"))
        }

        let rect = region ?? CGRect.infinite
        guard let image = CGWindowListCreateImage(rect, .optionOnScreenOnly, kCGNullWindowID, .bestResolution) else {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to capture screen"))
        }

        let bitmapRep = NSBitmapImageRep(cgImage: image)
        guard let pngData = bitmapRep.representation(using: .png, properties: [:]) else {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to encode PNG"))
        }

        return .success(CaptureResult(
            base64PNG: pngData.base64EncodedString(),
            width: image.width,
            height: image.height
        ))
    }
}
```

**Step 2: Register handler in BridgeServer**

Add to `BridgeServer.registerDefaultHandlers()`:
```swift
handlers[BridgeMethod.screenshot] = { params in
    let region: CGRect?
    if let x = params["x"] as? Double,
       let y = params["y"] as? Double,
       let w = params["width"] as? Double,
       let h = params["height"] as? Double {
        region = CGRect(x: x, y: y, width: w, height: h)
    } else {
        region = nil
    }
    switch ScreenCapture.capture(region: region) {
    case .success(let result):
        return .success(["image": result.base64PNG, "width": result.width, "height": result.height])
    case .failure(let error):
        return .failure(error)
    }
}
```

**Step 3: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add ScreenCapture with CoreGraphics"
```

---

### Task 7: Implement OCR Service

**Files:**
- Create: `apps/macos-native/Aleph/Desktop/OCRService.swift`

> **Reference:** `apps/desktop/src-tauri/src/bridge/perception.rs:176-337` (macOS Vision FFI)

**Step 1: Write implementation**

```swift
// Aleph/Desktop/OCRService.swift
import Vision
import CoreImage
import Foundation

/// OCR using Vision framework (native, no FFI needed).
/// Replaces ~160 lines of objc FFI in Tauri perception.rs.
enum OCRService {

    struct OCRLine {
        let text: String
        let confidence: Float
    }

    struct OCRResult {
        let text: String
        let lines: [OCRLine]
    }

    /// Perform OCR on a base64-encoded PNG image.
    static func recognize(imageBase64: String) async -> Result<OCRResult, BridgeServer.BridgeHandlerError> {
        guard let imageData = Data(base64Encoded: imageBase64) else {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Invalid base64 image data"))
        }

        guard let ciImage = CIImage(data: imageData) else {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to create image from data"))
        }

        // Create text recognition request
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.usesLanguageCorrection = true
        request.recognitionLanguages = ["zh-Hans", "en-US"]

        // Create handler and perform
        let handler = VNImageRequestHandler(ciImage: ciImage, options: [:])
        do {
            try handler.perform([request])
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "OCR failed: \(error.localizedDescription)"))
        }

        // Extract results
        guard let observations = request.results else {
            return .success(OCRResult(text: "", lines: []))
        }

        var lines: [OCRLine] = []
        var fullText = ""

        for observation in observations {
            guard let candidate = observation.topCandidates(1).first else { continue }
            lines.append(OCRLine(text: candidate.string, confidence: candidate.confidence))
            if !fullText.isEmpty { fullText += "\n" }
            fullText += candidate.string
        }

        return .success(OCRResult(text: fullText, lines: lines))
    }

    /// Capture screen and perform OCR in one step.
    static func captureAndRecognize(region: CGRect? = nil) async -> Result<OCRResult, BridgeServer.BridgeHandlerError> {
        switch ScreenCapture.capture(region: region) {
        case .success(let capture):
            return await recognize(imageBase64: capture.base64PNG)
        case .failure(let error):
            return .failure(error)
        }
    }
}
```

**Step 2: Register handler in BridgeServer**

```swift
handlers[BridgeMethod.ocr] = { params in
    // OCR needs async — wrap in a semaphore for sync handler
    let semaphore = DispatchSemaphore(value: 0)
    var ocrResult: Result<Any, BridgeHandlerError>?

    Task {
        let result: Result<OCRService.OCRResult, BridgeHandlerError>
        if let imageBase64 = params["image"] as? String {
            result = await OCRService.recognize(imageBase64: imageBase64)
        } else {
            result = await OCRService.captureAndRecognize()
        }
        switch result {
        case .success(let ocr):
            ocrResult = .success([
                "text": ocr.text,
                "lines": ocr.lines.map { ["text": $0.text, "confidence": $0.confidence] }
            ] as [String: Any])
        case .failure(let error):
            ocrResult = .failure(error)
        }
        semaphore.signal()
    }
    semaphore.wait()
    return ocrResult ?? .failure(BridgeHandlerError(code: BridgeErrorCode.internalError, message: "OCR timeout"))
}
```

**Step 3: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add OCRService with native Vision framework"
```

---

### Task 8: Implement AccessibilityService

**Files:**
- Create: `apps/macos-native/Aleph/Desktop/AccessibilityService.swift`

> **Reference:** `apps/desktop/src-tauri/src/bridge/perception.rs:536-712` (macOS AX API FFI)

**Step 1: Write implementation**

```swift
// Aleph/Desktop/AccessibilityService.swift
import ApplicationServices
import Foundation

/// Accessibility tree inspection using AXUIElement API (native, no FFI needed).
/// Replaces ~180 lines of C FFI in Tauri perception.rs.
enum AccessibilityService {

    /// Check if accessibility permission is granted
    static var hasPermission: Bool {
        AXIsProcessTrusted()
    }

    /// Request accessibility permission (shows system preferences)
    static func requestPermission() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): true] as CFDictionary
        AXIsProcessTrustedWithOptions(options)
    }

    /// Inspect accessibility tree of an app by bundle ID, or frontmost app if nil.
    static func inspect(bundleId: String? = nil, maxDepth: Int = 5) -> Result<[String: Any], BridgeServer.BridgeHandlerError> {
        guard hasPermission else {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Accessibility permission not granted"))
        }

        let pid: pid_t
        if let bundleId = bundleId {
            guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: bundleId).first else {
                return .failure(.init(code: BridgeErrorCode.internalError,
                                      message: "App not found: \(bundleId)"))
            }
            pid = app.processIdentifier
        } else {
            guard let app = NSWorkspace.shared.frontmostApplication else {
                return .failure(.init(code: BridgeErrorCode.internalError,
                                      message: "No frontmost application"))
            }
            pid = app.processIdentifier
        }

        let appElement = AXUIElementCreateApplication(pid)
        let tree = walkTree(element: appElement, depth: 0, maxDepth: maxDepth)
        return .success(tree)
    }

    // MARK: - Private

    private static func walkTree(element: AXUIElement, depth: Int, maxDepth: Int) -> [String: Any] {
        var result: [String: Any] = [:]

        // Role
        if let role = getAttribute(element, kAXRoleAttribute) as? String {
            result["role"] = role
        }

        // Title
        if let title = getAttribute(element, kAXTitleAttribute) as? String, !title.isEmpty {
            result["title"] = title
        }

        // Value
        if let value = getAttribute(element, kAXValueAttribute) as? String, !value.isEmpty {
            result["value"] = value
        }

        // Children (recursive, up to maxDepth)
        if depth < maxDepth,
           let children = getAttribute(element, kAXChildrenAttribute) as? [AXUIElement] {
            let childTrees = children.prefix(128).map { child in
                walkTree(element: child, depth: depth + 1, maxDepth: maxDepth)
            }
            if !childTrees.isEmpty {
                result["children"] = childTrees
            }
        }

        return result
    }

    private static func getAttribute(_ element: AXUIElement, _ attribute: String) -> CFTypeRef? {
        var value: CFTypeRef?
        let error = AXUIElementCopyAttributeValue(element, attribute as CFString, &value)
        return error == .success ? value : nil
    }
}
```

**Step 2: Register handler in BridgeServer**

```swift
handlers[BridgeMethod.axTree] = { params in
    let bundleId = params["bundle_id"] as? String
    let maxDepth = params["max_depth"] as? Int ?? 5
    switch AccessibilityService.inspect(bundleId: bundleId, maxDepth: maxDepth) {
    case .success(let tree):
        return .success(tree)
    case .failure(let error):
        return .failure(error)
    }
}
```

**Step 3: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add AccessibilityService with native AX API"
```

---

### Task 9: Implement InputAutomation

**Files:**
- Create: `apps/macos-native/Aleph/Desktop/InputAutomation.swift`

> **Reference:** `apps/desktop/src-tauri/src/bridge/action.rs:1-200` (click, type, key combo, scroll)

**Step 1: Write implementation**

```swift
// Aleph/Desktop/InputAutomation.swift
import CoreGraphics
import Foundation

/// Keyboard and mouse automation using CGEvent API (no enigo dependency).
/// Replaces enigo crate usage in Tauri action.rs.
enum InputAutomation {

    // MARK: - Mouse

    /// Click at screen coordinates.
    static func click(x: Double, y: Double, button: String = "left") -> Result<Any, BridgeServer.BridgeHandlerError> {
        let point = CGPoint(x: x, y: y)
        let (downType, upType): (CGEventType, CGEventType) = switch button {
        case "right":
            (.rightMouseDown, .rightMouseUp)
        case "middle":
            (.otherMouseDown, .otherMouseUp)
        default:
            (.leftMouseDown, .leftMouseUp)
        }

        let mouseButton: CGMouseButton = switch button {
        case "right": .right
        case "middle": .center
        default: .left
        }

        guard let downEvent = CGEvent(mouseEventSource: nil, mouseType: downType, mouseCursorPosition: point, mouseButton: mouseButton),
              let upEvent = CGEvent(mouseEventSource: nil, mouseType: upType, mouseCursorPosition: point, mouseButton: mouseButton) else {
            return .failure(.init(code: BridgeErrorCode.internalError, message: "Failed to create mouse events"))
        }

        downEvent.post(tap: .cghidEventTap)
        upEvent.post(tap: .cghidEventTap)
        return .success(["clicked": true, "x": x, "y": y])
    }

    /// Scroll in a direction.
    static func scroll(direction: String, amount: Int = 3) -> Result<Any, BridgeServer.BridgeHandlerError> {
        let (dx, dy): (Int32, Int32) = switch direction {
        case "up": (0, Int32(amount))
        case "down": (0, Int32(-amount))
        case "left": (Int32(amount), 0)
        case "right": (Int32(-amount), 0)
        default: (0, Int32(-amount))
        }

        guard let event = CGEvent(scrollWheelEvent2Source: nil, units: .line, wheelCount: 2, wheel1: dy, wheel2: dx) else {
            return .failure(.init(code: BridgeErrorCode.internalError, message: "Failed to create scroll event"))
        }
        event.post(tap: .cghidEventTap)
        return .success(["scrolled": true, "direction": direction])
    }

    // MARK: - Keyboard

    /// Type text string.
    static func typeText(_ text: String) -> Result<Any, BridgeServer.BridgeHandlerError> {
        for char in text {
            guard let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true) else { continue }
            event.keyboardSetUnicodeString(string: String(char))
            event.post(tap: .cghidEventTap)

            guard let upEvent = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: false) else { continue }
            upEvent.post(tap: .cghidEventTap)

            Thread.sleep(forTimeInterval: 0.01) // Brief delay between keystrokes
        }
        return .success(["typed": true, "length": text.count])
    }

    /// Press a key combination (e.g., Cmd+C, Ctrl+Alt+Delete).
    static func keyCombo(modifiers: [String], key: String) -> Result<Any, BridgeServer.BridgeHandlerError> {
        guard let keyCode = keyCodeFor(key) else {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Unknown key: \(key)"))
        }

        var flags: CGEventFlags = []
        for modifier in modifiers {
            switch modifier.lowercased() {
            case "meta", "command", "cmd", "super":
                flags.insert(.maskCommand)
            case "shift":
                flags.insert(.maskShift)
            case "control", "ctrl":
                flags.insert(.maskControl)
            case "alt", "option":
                flags.insert(.maskAlternate)
            default:
                break
            }
        }

        guard let downEvent = CGEvent(keyboardEventSource: nil, virtualKey: keyCode, keyDown: true),
              let upEvent = CGEvent(keyboardEventSource: nil, virtualKey: keyCode, keyDown: false) else {
            return .failure(.init(code: BridgeErrorCode.internalError, message: "Failed to create key events"))
        }

        downEvent.flags = flags
        upEvent.flags = flags
        downEvent.post(tap: .cghidEventTap)
        upEvent.post(tap: .cghidEventTap)

        return .success(["pressed": true, "key": key, "modifiers": modifiers])
    }

    // MARK: - Key Code Mapping

    private static func keyCodeFor(_ key: String) -> CGKeyCode? {
        if key.count == 1, let char = key.lowercased().first {
            return charToKeyCode[char]
        }
        return namedKeyToKeyCode[key.lowercased()]
    }

    private static let namedKeyToKeyCode: [String: CGKeyCode] = [
        "return": 0x24, "enter": 0x24,
        "tab": 0x30,
        "space": 0x31,
        "delete": 0x33, "backspace": 0x33,
        "escape": 0x35, "esc": 0x35,
        "left": 0x7B, "right": 0x7C,
        "down": 0x7D, "up": 0x7E,
        "f1": 0x7A, "f2": 0x78, "f3": 0x63, "f4": 0x76,
        "f5": 0x60, "f6": 0x61, "f7": 0x62, "f8": 0x64,
        "f9": 0x65, "f10": 0x6D, "f11": 0x67, "f12": 0x6F,
        "home": 0x73, "end": 0x77,
        "pageup": 0x74, "pagedown": 0x79,
        "forwarddelete": 0x75,
    ]

    private static let charToKeyCode: [Character: CGKeyCode] = [
        "a": 0x00, "s": 0x01, "d": 0x02, "f": 0x03,
        "h": 0x04, "g": 0x05, "z": 0x06, "x": 0x07,
        "c": 0x08, "v": 0x09, "b": 0x0B, "q": 0x0C,
        "w": 0x0D, "e": 0x0E, "r": 0x0F, "y": 0x10,
        "t": 0x11, "1": 0x12, "2": 0x13, "3": 0x14,
        "4": 0x15, "6": 0x16, "5": 0x17, "=": 0x18,
        "9": 0x19, "7": 0x1A, "-": 0x1B, "8": 0x1C,
        "0": 0x1D, "]": 0x1E, "o": 0x1F, "u": 0x20,
        "[": 0x21, "i": 0x22, "p": 0x23, "l": 0x25,
        "j": 0x26, "'": 0x27, "k": 0x28, ";": 0x29,
        "\\": 0x2A, ",": 0x2B, "/": 0x2C, "n": 0x2D,
        "m": 0x2E, ".": 0x2F, "`": 0x32,
    ]
}

private extension CGEvent {
    func keyboardSetUnicodeString(string: String) {
        let chars = Array(string.utf16)
        self.keyboardSetUnicodeString(stringLength: chars.count, unicodeString: chars)
    }
}
```

**Step 2: Register handlers in BridgeServer**

```swift
handlers[BridgeMethod.click] = { params in
    let x = params["x"] as? Double ?? 0
    let y = params["y"] as? Double ?? 0
    let button = params["button"] as? String ?? "left"
    return InputAutomation.click(x: x, y: y, button: button)
}

handlers[BridgeMethod.typeText] = { params in
    let text = params["text"] as? String ?? ""
    return InputAutomation.typeText(text)
}

handlers[BridgeMethod.keyCombo] = { params in
    let modifiers = params["modifiers"] as? [String] ?? []
    let key = params["key"] as? String ?? ""
    // Support legacy flat array format
    if modifiers.isEmpty, let keys = params["keys"] as? [String], keys.count >= 1 {
        let lastKey = keys.last!
        let mods = Array(keys.dropLast())
        return InputAutomation.keyCombo(modifiers: mods, key: lastKey)
    }
    return InputAutomation.keyCombo(modifiers: modifiers, key: key)
}

handlers[BridgeMethod.scroll] = { params in
    let direction = params["direction"] as? String ?? "down"
    let amount = params["amount"] as? Int ?? 3
    return InputAutomation.scroll(direction: direction, amount: amount)
}
```

**Step 3: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add InputAutomation with CGEvent API"
```

---

### Task 10: Implement WindowManager

**Files:**
- Create: `apps/macos-native/Aleph/Desktop/WindowManager.swift`

> **Reference:** `apps/desktop/src-tauri/src/bridge/action.rs:324-678`

**Step 1: Write implementation**

```swift
// Aleph/Desktop/WindowManager.swift
import Cocoa
import CoreGraphics

/// Window listing and focusing using CoreGraphics and NSRunningApplication.
/// Replaces CoreGraphics FFI in Tauri action.rs.
enum WindowManager {

    /// List all visible windows
    static func listWindows() -> Result<Any, BridgeServer.BridgeHandlerError> {
        guard let windowList = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else {
            return .failure(.init(code: BridgeErrorCode.internalError, message: "Failed to get window list"))
        }

        let windows: [[String: Any]] = windowList.compactMap { info in
            guard let pid = info[kCGWindowOwnerPID as String] as? pid_t,
                  let name = info[kCGWindowOwnerName as String] as? String,
                  let windowNumber = info[kCGWindowNumber as String] as? Int,
                  let layer = info[kCGWindowLayer as String] as? Int,
                  layer == 0 else { // Normal window layer only
                return nil
            }

            let bounds = info[kCGWindowBounds as String] as? [String: Any] ?? [:]
            let title = info[kCGWindowName as String] as? String ?? ""

            return [
                "pid": pid,
                "app_name": name,
                "window_id": windowNumber,
                "title": title,
                "bounds": bounds
            ]
        }

        return .success(["windows": windows])
    }

    /// Focus a window by PID
    static func focusWindow(pid: pid_t) -> Result<Any, BridgeServer.BridgeHandlerError> {
        guard let app = NSRunningApplication(processIdentifier: pid) else {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "No app found with PID \(pid)"))
        }

        let success = app.activate(options: [.activateIgnoringOtherApps])
        if success {
            return .success(["focused": true, "pid": pid])
        } else {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to focus app with PID \(pid)"))
        }
    }

    /// Launch an app by bundle ID
    static func launchApp(bundleId: String) -> Result<Any, BridgeServer.BridgeHandlerError> {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        process.arguments = ["-b", bundleId]

        do {
            try process.run()
            process.waitUntilExit()
            return .success(["launched": true, "bundle_id": bundleId])
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to launch \(bundleId): \(error.localizedDescription)"))
        }
    }
}
```

**Step 2: Register handlers in BridgeServer**

```swift
handlers[BridgeMethod.windowList] = { _ in WindowManager.listWindows() }
handlers[BridgeMethod.focusWindow] = { params in
    let pid = params["pid"] as? Int ?? 0
    return WindowManager.focusWindow(pid: pid_t(pid))
}
handlers[BridgeMethod.launchApp] = { params in
    let bundleId = params["bundle_id"] as? String ?? ""
    return WindowManager.launchApp(bundleId: bundleId)
}
```

**Step 3: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add WindowManager with CoreGraphics"
```

---

## Phase 5: UI Layer

### Task 11: Implement MenuBarController

**Files:**
- Create: `apps/macos-native/Aleph/UI/MenuBarController.swift`

> **Reference:** `apps/desktop/src-tauri/src/tray.rs`

**Step 1: Write implementation**

```swift
// Aleph/UI/MenuBarController.swift
import Cocoa
import SwiftUI

/// System menu bar (NSStatusItem) controller.
/// Replaces Tauri tray.rs implementation.
final class MenuBarController: NSObject, ObservableObject {

    private var statusItem: NSStatusItem?
    @Published var currentStatus: String = "idle"

    func setup() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)

        if let button = statusItem?.button {
            button.image = NSImage(systemSymbolName: "brain.head.profile", accessibilityDescription: "Aleph")
            button.toolTip = "Aleph - AI Assistant"
        }

        statusItem?.menu = buildMenu()
    }

    func updateStatus(_ status: String, tooltip: String? = nil) {
        currentStatus = status
        let tooltipText = tooltip ?? switch status {
        case "thinking": "Aleph - Thinking..."
        case "acting": "Aleph - Acting..."
        case "error": "Aleph - Error"
        default: "Aleph - AI Assistant"
        }
        statusItem?.button?.toolTip = tooltipText
    }

    // MARK: - Menu

    private func buildMenu() -> NSMenu {
        let menu = NSMenu()

        menu.addItem(NSMenuItem(title: "About Aleph", action: #selector(showAbout), keyEquivalent: ""))
        let versionItem = NSMenuItem(title: "Version 0.1.0", action: nil, keyEquivalent: "")
        versionItem.isEnabled = false
        menu.addItem(versionItem)

        menu.addItem(NSMenuItem.separator())

        let haloItem = NSMenuItem(title: "Show Halo", action: #selector(showHalo), keyEquivalent: "/")
        haloItem.keyEquivalentModifierMask = [.command, .option]
        menu.addItem(haloItem)

        menu.addItem(NSMenuItem.separator())

        let settingsItem = NSMenuItem(title: "Settings...", action: #selector(showSettings), keyEquivalent: ",")
        menu.addItem(settingsItem)

        menu.addItem(NSMenuItem.separator())

        let quitItem = NSMenuItem(title: "Quit Aleph", action: #selector(quitApp), keyEquivalent: "q")
        menu.addItem(quitItem)

        // Set target for all items
        for item in menu.items {
            item.target = self
        }

        return menu
    }

    // MARK: - Actions

    @objc private func showAbout() {
        // Will be wired to settings window navigation later
        NSApp.orderFrontStandardAboutPanel(nil)
    }

    @objc private func showHalo() {
        NotificationCenter.default.post(name: .showHalo, object: nil)
    }

    @objc private func showSettings() {
        NotificationCenter.default.post(name: .showSettings, object: nil)
    }

    @objc private func quitApp() {
        NSApplication.shared.terminate(nil)
    }
}

// MARK: - Notification Names

extension Notification.Name {
    static let showHalo = Notification.Name("com.aleph.showHalo")
    static let showSettings = Notification.Name("com.aleph.showSettings")
}
```

**Step 2: Wire into AppDelegate**

```swift
// Add to AppDelegate.swift
let menuBar = MenuBarController()

func applicationDidFinishLaunching(_ notification: Notification) {
    NSApp.setActivationPolicy(.accessory)
    menuBar.setup()
    // ... server start
}
```

**Step 3: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add MenuBarController with NSStatusItem"
```

---

### Task 12: Implement HaloWindow and SettingsWindow

**Files:**
- Create: `apps/macos-native/Aleph/UI/HaloWindow.swift`
- Create: `apps/macos-native/Aleph/UI/SettingsWindow.swift`

> **Reference:** `apps/desktop/src-tauri/src/commands/mod.rs:70-138` (Halo), `tauri.conf.json` (window config)

**Step 1: Write HaloWindow**

```swift
// Aleph/UI/HaloWindow.swift
import Cocoa
import WebKit

/// Floating Halo input window (NSPanel + WKWebView).
/// Replaces Tauri's "halo" WebView window config.
final class HaloWindow: NSObject {

    private var panel: NSPanel?
    private var webView: WKWebView?
    private var serverPort: Int = 18790

    func configure(serverPort: Int) {
        self.serverPort = serverPort
    }

    func show() {
        if panel == nil {
            createPanel()
        }
        guard let panel = panel, let screen = NSScreen.main else { return }

        // Position: centered horizontally, 30% from bottom
        let screenFrame = screen.visibleFrame
        let windowWidth: CGFloat = 800
        let windowHeight: CGFloat = 80
        let x = screenFrame.origin.x + (screenFrame.width - windowWidth) / 2
        let y = screenFrame.origin.y + (screenFrame.height * 0.3) - windowHeight / 2
        panel.setFrame(NSRect(x: x, y: y, width: windowWidth, height: windowHeight), display: true)

        panel.makeKeyAndOrderFront(nil)

        // Navigate to Halo UI
        if let url = URL(string: "http://127.0.0.1:\(serverPort)/halo") {
            webView?.load(URLRequest(url: url))
        }
    }

    func hide() {
        panel?.orderOut(nil)
    }

    func navigate(to url: URL) {
        webView?.load(URLRequest(url: url))
    }

    // MARK: - Private

    private func createPanel() {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 800, height: 80),
            styleMask: [.borderless, .nonactivatingPanel, .hudWindow],
            backing: .buffered,
            defer: false
        )
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]

        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

        let webView = WKWebView(frame: panel.contentView!.bounds, configuration: config)
        webView.autoresizingMask = [.width, .height]
        webView.setValue(false, forKey: "drawsBackground") // Transparent
        panel.contentView?.addSubview(webView)

        self.panel = panel
        self.webView = webView
    }
}
```

**Step 2: Write SettingsWindow**

```swift
// Aleph/UI/SettingsWindow.swift
import Cocoa
import WebKit

/// Settings window (NSWindow + WKWebView).
/// Replaces Tauri's "settings" WebView window config.
final class SettingsWindow: NSObject, NSWindowDelegate {

    private var window: NSWindow?
    private var webView: WKWebView?
    private var serverPort: Int = 18790
    private var savedFrame: NSRect?

    func configure(serverPort: Int) {
        self.serverPort = serverPort
    }

    func show() {
        if window == nil {
            createWindow()
        }
        guard let window = window else { return }

        // Restore saved position or center
        if let frame = savedFrame {
            window.setFrame(frame, display: true)
        } else {
            window.center()
        }

        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        // Navigate to Settings UI
        if let url = URL(string: "http://127.0.0.1:\(serverPort)/settings") {
            webView?.load(URLRequest(url: url))
        }
    }

    func hide() {
        savedFrame = window?.frame
        window?.orderOut(nil)
    }

    // NSWindowDelegate
    func windowWillClose(_ notification: Notification) {
        savedFrame = window?.frame
    }

    // MARK: - Private

    private func createWindow() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 900, height: 650),
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Aleph Settings"
        window.minSize = NSSize(width: 700, height: 500)
        window.delegate = self

        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

        let webView = WKWebView(frame: window.contentView!.bounds, configuration: config)
        webView.autoresizingMask = [.width, .height]
        window.contentView?.addSubview(webView)

        self.window = window
        self.webView = webView
    }
}
```

**Step 3: Wire windows into AppDelegate**

```swift
// Add to AppDelegate.swift
let haloWindow = HaloWindow()
let settingsWindow = SettingsWindow()

func applicationDidFinishLaunching(_ notification: Notification) {
    // ... existing setup ...

    // Wire notification observers
    NotificationCenter.default.addObserver(forName: .showHalo, object: nil, queue: .main) { [weak self] _ in
        self?.haloWindow.show()
    }
    NotificationCenter.default.addObserver(forName: .showSettings, object: nil, queue: .main) { [weak self] _ in
        self?.settingsWindow.show()
    }
}
```

**Step 4: Register WebView handlers in BridgeServer**

```swift
// In BridgeServer, add WebView control handlers
// These need a reference to the UI controllers (inject via closure or delegate)
handlers[BridgeMethod.webviewShow] = { params in
    let label = params["label"] as? String ?? "halo"
    let url = params["url"] as? String
    DispatchQueue.main.async {
        // Post notification to show window
        NotificationCenter.default.post(
            name: label == "settings" ? .showSettings : .showHalo,
            object: url
        )
    }
    return .success(["shown": true, "label": label])
}

handlers[BridgeMethod.webviewHide] = { params in
    let label = params["label"] as? String ?? "halo"
    DispatchQueue.main.async {
        NotificationCenter.default.post(
            name: Notification.Name("com.aleph.hideWebview"),
            object: label
        )
    }
    return .success(["hidden": true, "label": label])
}
```

**Step 5: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add HaloWindow (NSPanel) and SettingsWindow (NSWindow + WKWebView)"
```

---

### Task 13: Implement CanvasOverlay

**Files:**
- Create: `apps/macos-native/Aleph/Desktop/CanvasOverlay.swift`

> **Reference:** `apps/desktop/src-tauri/src/bridge/canvas.rs`

**Step 1: Write implementation**

```swift
// Aleph/Desktop/CanvasOverlay.swift
import Cocoa
import WebKit

/// Transparent overlay panel for A2UI canvas rendering.
/// Replaces Tauri canvas.rs WebView window.
final class CanvasOverlay: NSObject {

    private var panel: NSPanel?
    private var webView: WKWebView?

    func show(html: String, position: CGRect) {
        if panel == nil { createPanel() }
        guard let panel = panel, let webView = webView else { return }

        panel.setFrame(NSRect(x: position.origin.x, y: position.origin.y,
                              width: position.width, height: position.height), display: true)

        // Load HTML via data URI
        let base64 = Data(html.utf8).base64EncodedString()
        let dataURI = "data:text/html;base64,\(base64)"
        if let url = URL(string: dataURI) {
            webView.load(URLRequest(url: url))
        }

        // Inject A2UI handler after load
        let js = """
        window.alephApplyPatch = function(patch) {
            for (const p of patch) {
                if (p.type === 'surfaceUpdate' && p.content) {
                    document.body.innerHTML = p.content;
                }
            }
        };
        """
        webView.evaluateJavaScript(js)

        panel.makeKeyAndOrderFront(nil)
    }

    func hide() {
        panel?.orderOut(nil)
    }

    func update(patch: [[String: Any]]) {
        guard let webView = webView else { return }
        if let jsonData = try? JSONSerialization.data(withJSONObject: patch),
           let jsonString = String(data: jsonData, encoding: .utf8) {
            webView.evaluateJavaScript("window.alephApplyPatch(\(jsonString))")
        }
    }

    // MARK: - Private

    private func createPanel() {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.isFloatingPanel = true
        panel.level = .screenSaver
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.ignoresMouseEvents = true

        let config = WKWebViewConfiguration()
        let webView = WKWebView(frame: panel.contentView!.bounds, configuration: config)
        webView.autoresizingMask = [.width, .height]
        webView.setValue(false, forKey: "drawsBackground")
        panel.contentView?.addSubview(webView)

        self.panel = panel
        self.webView = webView
    }
}
```

**Step 2: Register handlers in BridgeServer**

```swift
// Canvas handlers (need CanvasOverlay reference)
handlers[BridgeMethod.canvasShow] = { params in
    let html = params["html"] as? String ?? ""
    let x = params["x"] as? Double ?? 0
    let y = params["y"] as? Double ?? 0
    let w = params["width"] as? Double ?? 400
    let h = params["height"] as? Double ?? 300
    DispatchQueue.main.async {
        // Post notification with canvas data
        NotificationCenter.default.post(
            name: Notification.Name("com.aleph.canvasShow"),
            object: nil,
            userInfo: ["html": html, "x": x, "y": y, "width": w, "height": h]
        )
    }
    return .success(["shown": true])
}

handlers[BridgeMethod.canvasHide] = { _ in
    DispatchQueue.main.async {
        NotificationCenter.default.post(name: Notification.Name("com.aleph.canvasHide"), object: nil)
    }
    return .success(["hidden": true])
}

handlers[BridgeMethod.canvasUpdate] = { params in
    let patch = params["patch"] as? [[String: Any]] ?? []
    DispatchQueue.main.async {
        NotificationCenter.default.post(
            name: Notification.Name("com.aleph.canvasUpdate"),
            object: nil,
            userInfo: ["patch": patch]
        )
    }
    return .success(["updated": true])
}
```

**Step 3: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add CanvasOverlay (NSPanel + WKWebView)"
```

---

### Task 14: Implement GlobalShortcuts

**Files:**
- Create: `apps/macos-native/Aleph/UI/GlobalShortcuts.swift`

> **Reference:** `apps/desktop/src-tauri/src/shortcuts.rs`

**Step 1: Write implementation**

```swift
// Aleph/UI/GlobalShortcuts.swift
import Cocoa
import Carbon

/// Global keyboard shortcut registration using CGEvent tap.
/// Replaces tauri_plugin_global_shortcut.
final class GlobalShortcuts {

    private var eventTap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?

    /// Register Cmd+Opt+/ to show Halo
    func register() {
        let mask: CGEventMask = (1 << CGEventType.keyDown.rawValue)

        guard let tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .defaultTap,
            eventsOfInterest: mask,
            callback: { _, _, event, _ -> Unmanaged<CGEvent>? in
                let keyCode = event.getIntegerValueField(.keyboardEventKeycode)
                let flags = event.flags

                // Cmd+Opt+/ (keycode 0x2C = /)
                if keyCode == 0x2C &&
                   flags.contains(.maskCommand) &&
                   flags.contains(.maskAlternate) {
                    DispatchQueue.main.async {
                        NotificationCenter.default.post(name: .showHalo, object: nil)
                    }
                    return nil // Consume event
                }
                return Unmanaged.passRetained(event)
            },
            userInfo: nil
        ) else {
            print("Failed to create event tap — accessibility permission required")
            return
        }

        self.eventTap = tap
        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        self.runLoopSource = source
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
    }

    func unregister() {
        if let tap = eventTap {
            CGEvent.tapEnable(tap: tap, enable: false)
        }
        if let source = runLoopSource {
            CFRunLoopRemoveSource(CFRunLoopGetCurrent(), source, .commonModes)
        }
        eventTap = nil
        runLoopSource = nil
    }

    deinit {
        unregister()
    }
}
```

**Step 2: Wire into AppDelegate**

```swift
let shortcuts = GlobalShortcuts()

func applicationDidFinishLaunching(_ notification: Notification) {
    // ... existing setup ...
    shortcuts.register()
}
```

**Step 3: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: add GlobalShortcuts with CGEvent tap"
```

---

## Phase 6: Tauri Changes

### Task 15: Remove macOS Target from Tauri

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml` — Remove macOS dependencies
- Modify: `apps/desktop/src-tauri/tauri.conf.json` — Remove dmg target and macOS config
- Modify: `apps/desktop/src-tauri/src/bridge/perception.rs` — Remove macOS OCR and AX code
- Modify: `apps/desktop/src-tauri/src/bridge/action.rs` — Remove macOS window/app launch code
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs` — Remove macOS capability declarations

**Step 1: Update Cargo.toml**

Remove the macOS dependencies section:
```toml
# DELETE this entire block:
[target.'cfg(target_os = "macos")'.dependencies]
cocoa = "0.26"
objc = "0.2"
core-graphics = "0.24"
core-foundation = "0.10"
```

Also in Tauri features, remove `macos-private-api`:
```toml
tauri = { version = "2", features = ["tray-icon", "image-png"] }
```

**Step 2: Update tauri.conf.json**

```json
{
  "app": {
    "macOSPrivateApi": false,
    ...
  },
  "bundle": {
    "targets": ["nsis", "deb", "appimage"],
    "macOS": {}  // Remove macOS-specific config or keep empty
  }
}
```

**Step 3: Remove macOS-specific code from perception.rs**

Remove all `#[cfg(target_os = "macos")]` blocks for OCR and AX API. Keep Windows and Linux implementations.

**Step 4: Remove macOS-specific code from action.rs**

Remove macOS window management code (CoreGraphics imports, NSRunningApplication). Keep Windows (`EnumWindows`) and Linux (`wmctrl`) implementations.

**Step 5: Update handshake in mod.rs**

Remove the `#[cfg(target_os = "macos")]` block that adds ocr and ax_inspect capabilities.

**Step 6: Verify build for Linux/Windows targets**

```bash
cd apps/desktop && cargo check
```
Expected: Compiles without macOS-specific errors (may need cross-compilation or conditional check).

**Step 7: Commit**

```bash
git add apps/desktop/
git commit -m "desktop: remove macOS target, keep Linux/Windows only"
```

---

### Task 16: Add Server Embedding to Tauri

**Files:**
- Create: `apps/desktop/src-tauri/src/server_manager.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` — Add server lifecycle
- Modify: `apps/desktop/src-tauri/tauri.conf.json` — Add server binary as resource

**Step 1: Create ServerManager for Tauri**

```rust
// apps/desktop/src-tauri/src/server_manager.rs
use std::path::PathBuf;
use std::process::{Child, Command};
use tracing::{error, info, warn};

/// Manages the lifecycle of the embedded aleph-server binary.
pub struct ServerManager {
    process: Option<Child>,
    socket_path: PathBuf,
}

impl ServerManager {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            process: None,
            socket_path,
        }
    }

    /// Start the embedded aleph-server.
    /// Looks for the binary in the Tauri resource directory.
    pub fn start(&mut self, resource_dir: &std::path::Path) -> Result<(), String> {
        let server_bin = resource_dir.join("aleph-server");

        if !server_bin.exists() {
            return Err(format!("Server binary not found at {:?}", server_bin));
        }

        info!("Starting aleph-server from {:?}", server_bin);

        // Ensure parent directory exists
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        // Remove stale socket
        std::fs::remove_file(&self.socket_path).ok();

        let child = Command::new(&server_bin)
            .args(["--bridge-mode", "--socket", &self.socket_path.to_string_lossy()])
            .spawn()
            .map_err(|e| format!("Failed to start server: {}", e))?;

        info!("aleph-server started (PID: {})", child.id());
        self.process = Some(child);

        // Wait for socket to be ready
        self.wait_for_ready()?;

        Ok(())
    }

    /// Stop the server gracefully.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.process.take() {
            info!("Stopping aleph-server (PID: {})", child.id());

            // Try graceful shutdown first
            #[cfg(unix)]
            {
                unsafe { libc::kill(child.id() as i32, libc::SIGTERM); }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }

            // Force kill if still running
            match child.try_wait() {
                Ok(Some(_)) => info!("Server stopped gracefully"),
                _ => {
                    warn!("Force killing server");
                    child.kill().ok();
                    child.wait().ok();
                }
            }
        }

        // Clean up socket
        std::fs::remove_file(&self.socket_path).ok();
    }

    fn wait_for_ready(&self) -> Result<(), String> {
        for _ in 0..50 {
            if self.socket_path.exists() {
                // Try connecting
                match std::os::unix::net::UnixStream::connect(&self.socket_path) {
                    Ok(_) => return Ok(()),
                    Err(_) => {}
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err("Server did not become ready within 10 seconds".into())
    }
}

impl Drop for ServerManager {
    fn drop(&mut self) {
        self.stop();
    }
}
```

**Step 2: Wire into lib.rs**

Add server startup in Tauri setup:
```rust
mod server_manager;

// In setup:
let resource_dir = app.path().resource_dir().unwrap();
let socket_path = aleph_protocol::desktop_bridge::default_socket_path();
let mut server_mgr = server_manager::ServerManager::new(socket_path);
if let Err(e) = server_mgr.start(&resource_dir) {
    error!("Failed to start embedded server: {}", e);
}
// Store server_mgr in app state for cleanup
```

**Step 3: Update tauri.conf.json**

```json
{
  "bundle": {
    "resources": ["binaries/aleph-server"]
  }
}
```

**Step 4: Commit**

```bash
git add apps/desktop/
git commit -m "desktop: add ServerManager for embedded aleph-server"
```

---

## Phase 7: Build Scripts & Distribution

### Task 17: Create macOS Build Script

**Files:**
- Create: `scripts/build-macos.sh`

**Step 1: Write build script**

```bash
#!/bin/bash
# scripts/build-macos.sh
# Build macOS Aleph.app with embedded aleph-server
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
MACOS_DIR="$PROJECT_ROOT/apps/macos-native"
BUILD_DIR="$PROJECT_ROOT/target/macos-app"

echo "=== Building aleph-server (universal binary) ==="

# Build for both architectures
cargo build --bin aleph-server --features control-plane --release --target aarch64-apple-darwin
cargo build --bin aleph-server --features control-plane --release --target x86_64-apple-darwin

# Create universal binary
mkdir -p "$BUILD_DIR"
lipo -create \
    "$PROJECT_ROOT/target/aarch64-apple-darwin/release/aleph-server" \
    "$PROJECT_ROOT/target/x86_64-apple-darwin/release/aleph-server" \
    -output "$BUILD_DIR/aleph-server"

echo "=== Universal binary created ($(du -h "$BUILD_DIR/aleph-server" | cut -f1)) ==="

# Copy server binary to Xcode project resources
mkdir -p "$MACOS_DIR/Aleph/Resources"
cp "$BUILD_DIR/aleph-server" "$MACOS_DIR/Aleph/Resources/"

echo "=== Building Aleph.app ==="

# Build with Xcode
cd "$MACOS_DIR"
xcodebuild \
    -project Aleph.xcodeproj \
    -scheme Aleph \
    -configuration Release \
    -derivedDataPath "$BUILD_DIR/DerivedData" \
    clean build

# Locate built app
APP_PATH="$BUILD_DIR/DerivedData/Build/Products/Release/Aleph.app"

if [ ! -d "$APP_PATH" ]; then
    echo "ERROR: Aleph.app not found at $APP_PATH"
    exit 1
fi

echo "=== Aleph.app built successfully ==="
echo "Location: $APP_PATH"
echo "Size: $(du -sh "$APP_PATH" | cut -f1)"

# Optional: Create DMG
if command -v create-dmg &> /dev/null; then
    echo "=== Creating DMG ==="
    DMG_PATH="$BUILD_DIR/Aleph.dmg"
    create-dmg \
        --volname "Aleph" \
        --window-pos 200 120 \
        --window-size 600 400 \
        --icon-size 100 \
        --app-drop-link 450 185 \
        "$DMG_PATH" \
        "$APP_PATH"
    echo "DMG: $DMG_PATH"
fi

echo "=== Done ==="
```

**Step 2: Make executable and commit**

```bash
chmod +x scripts/build-macos.sh
git add scripts/build-macos.sh
git commit -m "scripts: add macOS build script (cargo + xcodebuild + lipo)"
```

---

### Task 18: Create Server Install Script

**Files:**
- Create: `scripts/install.sh`

**Step 1: Write install script**

```bash
#!/bin/bash
# scripts/install.sh
# One-line installer: curl -fsSL https://get.aleph.dev | bash
set -euo pipefail

REPO="rootazero/Aleph"
INSTALL_DIR="/usr/local/bin"
BINARY_NAME="aleph-server"

# Detect platform
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
    darwin) PLATFORM="darwin" ;;
    linux)  PLATFORM="linux" ;;
    *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
    x86_64|amd64)  ARCH="x86_64" ;;
    arm64|aarch64) ARCH="aarch64" ;;
    *)             echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

ASSET_NAME="${BINARY_NAME}-${PLATFORM}-${ARCH}"
echo "Detected: $PLATFORM/$ARCH"

# Get latest release URL
LATEST_URL="https://api.github.com/repos/$REPO/releases/latest"
echo "Fetching latest release..."
DOWNLOAD_URL=$(curl -fsSL "$LATEST_URL" | grep "browser_download_url.*$ASSET_NAME\"" | head -1 | cut -d'"' -f4)

if [ -z "$DOWNLOAD_URL" ]; then
    echo "ERROR: No binary found for $ASSET_NAME"
    echo "Check releases at https://github.com/$REPO/releases"
    exit 1
fi

# Download
echo "Downloading $ASSET_NAME..."
TMP_FILE=$(mktemp)
curl -fsSL -o "$TMP_FILE" "$DOWNLOAD_URL"
chmod +x "$TMP_FILE"

# Install
echo "Installing to $INSTALL_DIR/$BINARY_NAME..."
if [ -w "$INSTALL_DIR" ]; then
    mv "$TMP_FILE" "$INSTALL_DIR/$BINARY_NAME"
else
    sudo mv "$TMP_FILE" "$INSTALL_DIR/$BINARY_NAME"
fi

# Create data directory
mkdir -p "$HOME/.aleph"

echo ""
echo "aleph-server installed successfully!"
echo "Run: aleph-server"
echo ""

# Offer to install as service
read -p "Install as system service? [y/N] " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    if [ "$PLATFORM" = "darwin" ]; then
        # macOS launchd
        PLIST="$HOME/Library/LaunchAgents/com.aleph.server.plist"
        cat > "$PLIST" << PLISTEOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.aleph.server</string>
    <key>ProgramArguments</key>
    <array>
        <string>$INSTALL_DIR/$BINARY_NAME</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$HOME/.aleph/server.log</string>
    <key>StandardErrorPath</key>
    <string>$HOME/.aleph/server.err</string>
</dict>
</plist>
PLISTEOF
        launchctl load "$PLIST"
        echo "Service installed. Use: launchctl start com.aleph.server"
    else
        # Linux systemd
        SERVICE_FILE="$HOME/.config/systemd/user/aleph-server.service"
        mkdir -p "$(dirname "$SERVICE_FILE")"
        cat > "$SERVICE_FILE" << SERVICEEOF
[Unit]
Description=Aleph AI Server
After=network.target

[Service]
ExecStart=$INSTALL_DIR/$BINARY_NAME
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
SERVICEEOF
        systemctl --user daemon-reload
        systemctl --user enable aleph-server
        systemctl --user start aleph-server
        echo "Service installed. Use: systemctl --user status aleph-server"
    fi
fi
```

**Step 2: Make executable and commit**

```bash
chmod +x scripts/install.sh
git add scripts/install.sh
git commit -m "scripts: add server install script with service setup"
```

---

## Phase 8: CI/CD Workflows

### Task 19: Create Server Release Workflow

**Files:**
- Create: `.github/workflows/server-release.yml`

**Step 1: Write workflow**

```yaml
# .github/workflows/server-release.yml
name: Server Release

on:
  push:
    tags: ['v*']
  workflow_dispatch:

jobs:
  build:
    strategy:
      matrix:
        include:
          - os: macos-latest
            target: aarch64-apple-darwin
            asset: aleph-server-darwin-aarch64
          - os: macos-latest
            target: x86_64-apple-darwin
            asset: aleph-server-darwin-x86_64
          - os: ubuntu-22.04
            target: x86_64-unknown-linux-gnu
            asset: aleph-server-linux-x86_64
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            asset: aleph-server-windows-x86_64.exe
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: Build server
        run: cargo build --bin aleph-server --features control-plane --release --target ${{ matrix.target }}
      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.asset }}
          path: target/${{ matrix.target }}/release/aleph-server${{ matrix.os == 'windows-latest' && '.exe' || '' }}

  release:
    needs: build
    runs-on: ubuntu-latest
    if: startsWith(github.ref, 'refs/tags/')
    steps:
      - uses: actions/download-artifact@v4
      - name: Create Release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            aleph-server-darwin-aarch64/*
            aleph-server-darwin-x86_64/*
            aleph-server-linux-x86_64/*
            aleph-server-windows-x86_64.exe/*
```

**Step 2: Commit**

```bash
git add .github/workflows/server-release.yml
git commit -m "ci: add server release workflow (multi-platform)"
```

---

### Task 20: Create macOS App Release Workflow

**Files:**
- Create: `.github/workflows/macos-app-release.yml`

**Step 1: Write workflow**

```yaml
# .github/workflows/macos-app-release.yml
name: macOS App Release

on:
  push:
    tags: ['v*']
  workflow_dispatch:

jobs:
  build:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-apple-darwin,x86_64-apple-darwin

      - name: Build universal server binary
        run: |
          cargo build --bin aleph-server --features control-plane --release --target aarch64-apple-darwin
          cargo build --bin aleph-server --features control-plane --release --target x86_64-apple-darwin
          lipo -create \
            target/aarch64-apple-darwin/release/aleph-server \
            target/x86_64-apple-darwin/release/aleph-server \
            -output apps/macos-native/Aleph/Resources/aleph-server

      - name: Build Aleph.app
        run: |
          cd apps/macos-native
          xcodebuild \
            -project Aleph.xcodeproj \
            -scheme Aleph \
            -configuration Release \
            -derivedDataPath build \
            clean build

      - name: Create DMG
        run: |
          APP_PATH="apps/macos-native/build/Build/Products/Release/Aleph.app"
          hdiutil create -volname "Aleph" -srcfolder "$APP_PATH" -ov -format UDZO Aleph.dmg

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: Aleph.dmg
          path: Aleph.dmg

  release:
    needs: build
    runs-on: ubuntu-latest
    if: startsWith(github.ref, 'refs/tags/')
    steps:
      - uses: actions/download-artifact@v4
      - name: Attach to Release
        uses: softprops/action-gh-release@v2
        with:
          files: Aleph.dmg/*
```

**Step 2: Commit**

```bash
git add .github/workflows/macos-app-release.yml
git commit -m "ci: add macOS app release workflow (Xcode + universal server)"
```

---

### Task 21: Update Tauri Release Workflow

**Files:**
- Modify: `.github/workflows/tauri-release.yml` — Remove macOS, add server embedding

**Step 1: Update workflow**

Update the existing workflow to:
1. Remove macOS from the build matrix
2. Add a step to build and copy aleph-server into Tauri resources before `cargo tauri build`
3. Fix the path from `platforms/tauri/` to `apps/desktop/`

**Step 2: Commit**

```bash
git add .github/workflows/tauri-release.yml
git commit -m "ci: update Tauri release — remove macOS, add server embedding"
```

---

## Phase 9: Integration & Final Assembly

### Task 22: Wire All Components in AppDelegate

**Files:**
- Modify: `apps/macos-native/Aleph/AppDelegate.swift`

**Step 1: Full AppDelegate implementation**

```swift
// Aleph/AppDelegate.swift
import Cocoa

class AppDelegate: NSObject, NSApplicationDelegate {

    let serverManager = ServerManager()
    let menuBar = MenuBarController()
    let haloWindow = HaloWindow()
    let settingsWindow = SettingsWindow()
    let canvasOverlay = CanvasOverlay()
    let shortcuts = GlobalShortcuts()
    var bridgeServer: BridgeServer?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)

        // 1. Setup menu bar
        menuBar.setup()

        // 2. Register global shortcuts
        shortcuts.register()

        // 3. Wire notification observers
        setupNotificationObservers()

        // 4. Start server and bridge
        Task {
            do {
                try await serverManager.start()

                // Configure UI with server port
                let port = 18790 // Default Control Plane port
                haloWindow.configure(serverPort: port)
                settingsWindow.configure(serverPort: port)

                // Start bridge server
                let bridge = BridgeServer()
                registerDesktopHandlers(bridge)
                try await bridge.start()
                self.bridgeServer = bridge

            } catch {
                print("Startup error: \(error)")
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationWillTerminate(_ notification: Notification) {
        Task {
            await bridgeServer?.stop()
            await serverManager.stop()
        }
        shortcuts.unregister()
    }

    // MARK: - Private

    private func setupNotificationObservers() {
        NotificationCenter.default.addObserver(forName: .showHalo, object: nil, queue: .main) { [weak self] _ in
            self?.haloWindow.show()
        }
        NotificationCenter.default.addObserver(forName: .showSettings, object: nil, queue: .main) { [weak self] _ in
            self?.settingsWindow.show()
        }
        NotificationCenter.default.addObserver(forName: .init("com.aleph.canvasShow"), object: nil, queue: .main) { [weak self] notification in
            guard let info = notification.userInfo,
                  let html = info["html"] as? String,
                  let x = info["x"] as? Double,
                  let y = info["y"] as? Double,
                  let w = info["width"] as? Double,
                  let h = info["height"] as? Double else { return }
            self?.canvasOverlay.show(html: html, position: CGRect(x: x, y: y, width: w, height: h))
        }
        NotificationCenter.default.addObserver(forName: .init("com.aleph.canvasHide"), object: nil, queue: .main) { [weak self] _ in
            self?.canvasOverlay.hide()
        }
        NotificationCenter.default.addObserver(forName: .init("com.aleph.canvasUpdate"), object: nil, queue: .main) { [weak self] notification in
            guard let patch = notification.userInfo?["patch"] as? [[String: Any]] else { return }
            self?.canvasOverlay.update(patch: patch)
        }
    }

    private func registerDesktopHandlers(_ bridge: BridgeServer) {
        // Desktop capability handlers are registered in BridgeServer.registerDefaultHandlers()
        // Additional handlers that need UI references:

        Task {
            await bridge.register(method: BridgeMethod.trayUpdateStatus) { [weak self] params in
                let status = params["status"] as? String ?? "idle"
                let tooltip = params["tooltip"] as? String
                DispatchQueue.main.async {
                    self?.menuBar.updateStatus(status, tooltip: tooltip)
                }
                return .success(["updated": true, "status": status])
            }
        }
    }
}
```

**Step 2: Verify the app builds and runs**

```bash
cd apps/macos-native && xcodebuild -scheme Aleph -configuration Debug build
```

**Step 3: Commit**

```bash
git add apps/macos-native/
git commit -m "macos: wire all components in AppDelegate — full integration"
```

---

### Task 23: Clean Up Deprecated macOS App

**Files:**
- Delete: `apps/macos/` (deprecated Swift app)

**Step 1: Verify new app is working**

Build and test the new macOS app manually.

**Step 2: Remove deprecated directory**

```bash
git rm -r apps/macos/
git commit -m "macos: remove deprecated Swift app (replaced by apps/macos-native)"
```

---

### Task 24: Update Documentation

**Files:**
- Modify: `CLAUDE.md` — Update project structure section
- Modify: `docs/reference/SERVER_DEVELOPMENT.md` — Add distribution commands

**Step 1: Update CLAUDE.md project structure**

Add `apps/macos-native/` to the project structure diagram. Update build commands section.

**Step 2: Update SERVER_DEVELOPMENT.md**

Add new build and distribution commands:
```bash
# macOS native app
./scripts/build-macos.sh

# Pure server install
curl -fsSL https://get.aleph.dev | bash

# Tauri desktop (Linux/Windows)
cd apps/desktop && cargo tauri build
```

**Step 3: Commit**

```bash
git add CLAUDE.md docs/reference/SERVER_DEVELOPMENT.md
git commit -m "docs: update project structure and build commands for dual distribution"
```

---

## Summary

| Phase | Tasks | Key Deliverables |
|-------|-------|-----------------|
| **1. Foundation** | 1-2 | Xcode project scaffold, ServerPaths |
| **2. Server Lifecycle** | 3 | ServerManager (start/stop/monitor) |
| **3. Bridge Server** | 4-5 | BridgeProtocol, BridgeServer (UDS) |
| **4. Desktop Capabilities** | 6-10 | ScreenCapture, OCR, AX, Input, Window |
| **5. UI Layer** | 11-14 | MenuBar, Halo, Settings, Shortcuts |
| **6. Tauri Changes** | 15-16 | Remove macOS target, add server embedding |
| **7. Build Scripts** | 17-18 | build-macos.sh, install.sh |
| **8. CI/CD** | 19-21 | 3 GitHub Actions workflows |
| **9. Integration** | 22-24 | Full wiring, cleanup, docs |

**Total: 24 tasks across 9 phases.**
