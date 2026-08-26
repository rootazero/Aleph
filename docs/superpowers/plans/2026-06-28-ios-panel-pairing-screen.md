# iOS Panel Pairing Screen Implementation Plan

> **⚠️ Superseded 2026-08-24 — the committed `Info.plist` this document edits no longer exists.**
> `mobile/ios/AlephPaneliOS/Resources/Info.plist` is xcodegen *output* and is now
> gitignored beside the generated `.xcodeproj`; `project.yml`'s `info.properties`
> block is the only source, and there is nothing to restore before a commit. Every
> step below that stages that file, or that asks a regeneration to preserve its
> `${ALEPH_VERSION}` / `${ALEPH_BUILD}` placeholders, describes the world as it was
> — the current one is stated once, in `mobile/ios/README.md` and
> `mobile/ios/.gitignore`. Kept as the record of what was done: do not re-add the
> file by following it.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the iOS Panel dev/sim harness into a shippable product by adding a native pairing screen (server URL + token), Keychain-backed storage, reachability-probed connect, a正式 Bundle ID, and VERSION-driven version numbers — zero changes to the WASM panel.

**Architecture:** A thin SwiftUI shell over `WKWebView`. An `AppState` (ObservableObject) drives one of two screens — a native `PairingView` (transport config only) or the `PanelWebView` loading the WASM panel. Connection target persists in Keychain; a TCP reachability probe gates every navigation. Address parsing/probe mirror the desktop lite shell (`desktop/shell/src/connection.rs` + `connect_setup.rs`) so both shells share one onboarding format (R6).

**Tech Stack:** Swift 5.9, SwiftUI, WebKit, Network framework (`NWConnection`/`NWListener`), Security framework (Keychain), Swift Testing (`import Testing`), xcodegen, xcodebuild (CLI, no Xcode GUI).

## Global Constraints

- **Deployment / language:** iOS 17.0 deployment target; `SWIFT_VERSION 5.9` (from `project.yml`).
- **R2/R4 boundary (HARD):** native code does **transport config only** (connect-where / probe / persist). All app UI/settings stay in the WASM panel. **Zero changes to `interfaces/webchat/`.**
- **Secrets:** the full pairing URL (contains `?token=`) is stored in **Keychain** (`kSecClassGenericPassword`), never `UserDefaults`, never logged/printed.
- **Address parse rules (mirror desktop `ConnectionTarget::parse`):** accept `host` / `host:port` / `http(s)://host[/path][?token=…]`; default scheme `http`; default port `18790`; reject empty and non-`http(s)` schemes.
- **Versioning:** Bundle ID = `ai.aleph.panel` (drop `.iossim`); `CFBundleShortVersionString`/`CFBundleVersion` come from the repo `VERSION` file (CalVer `YY.M.D`) — no hardcoded version.
- **Out of scope (do NOT touch):** iPad device family, `Info.plist` ATS (`NSAllowsArbitraryLoads`), CI/signing/distribution, QR/Bonjour discovery, in-panel "change server" button.
- **Tests:** Swift Testing (`@Test` / `#expect`). Run via `xcodebuild test`. Regenerate the project with **bare `xcodegen generate`** for test tasks (no token fetch needed); use `generate.sh` only for connection QA.
- **Prerequisites (verify once before Task 1):** `xcodegen`, `xcodebuild`, and at least one booted iOS Simulator. Pick an available device with `xcrun simctl list devices available` — examples below use `iPhone 16`; substitute a device that exists on your machine (per project memory, iOS 26.2 sims are reliable; 27.0 simctl can hang).
- **Commit style:** `ios: <description>` (English; attribution disabled globally — no Co-Authored-By trailer).
- **Working dir for all `xcodegen`/`xcodebuild` commands:** `mobile/ios`.

---

### Task 1: Unit test target + smoke test

Establishes the Swift Testing infrastructure so later tasks can do TDD. No product code yet.

**Files:**
- Modify: `mobile/ios/project.yml` (add `AlephPaneliOSTests` target + scheme test action)
- Create: `mobile/ios/AlephPaneliOSTests/SmokeTests.swift`

**Interfaces:**
- Consumes: nothing.
- Produces: a hosted unit-test target `AlephPaneliOSTests` runnable via `xcodebuild test -scheme AlephPaneliOS`. Later tasks add `*.swift` test files under `AlephPaneliOSTests/` (auto-included by the target's `sources`).

- [ ] **Step 1: Add the test target to `project.yml`**

In `mobile/ios/project.yml`, add a new entry under `targets:` (sibling of `AlephPaneliOS:`):

```yaml
  AlephPaneliOSTests:
    type: bundle.unit-test
    platform: iOS
    sources:
      - AlephPaneliOSTests
    dependencies:
      - target: AlephPaneliOS
    settings:
      base:
        TEST_HOST: "$(BUILT_PRODUCTS_DIR)/AlephPaneliOS.app/AlephPaneliOS"
        BUNDLE_LOADER: "$(TEST_HOST)"
        PRODUCT_BUNDLE_IDENTIFIER: ai.aleph.panel.iossim.tests
```

And extend the existing `schemes.AlephPaneliOS` block with a `test:` action (place it between `build:` and `run:`):

```yaml
schemes:
  AlephPaneliOS:
    build:
      targets:
        AlephPaneliOS: all
    test:
      targets:
        - AlephPaneliOSTests
    run:
      config: Debug
      environmentVariables:
        - variable: PANEL_URL
          value: ${PANEL_URL}
          isEnabled: true
```

(Keep the existing `run:`/`environmentVariables` exactly as they are — only the `test:` block is new.)

- [ ] **Step 2: Write a smoke test**

Create `mobile/ios/AlephPaneliOSTests/SmokeTests.swift`:

```swift
import Testing

@Suite struct SmokeTests {
    @Test("test target builds and runs")
    func smoke() {
        #expect(1 + 1 == 2)
    }
}
```

- [ ] **Step 3: Regenerate the project**

Run (in `mobile/ios`): `xcodegen generate`
Expected: `Created project at AlephPaneliOS.xcodeproj` with no errors.

- [ ] **Step 4: Run the test to verify the harness works**

Run: `xcodebuild test -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -destination 'platform=iOS Simulator,name=iPhone 16'`
Expected: build succeeds; `SmokeTests.smoke` passes; `** TEST SUCCEEDED **`.

- [ ] **Step 5: Commit**

```bash
git add -A mobile/ios/project.yml mobile/ios/AlephPaneliOSTests/SmokeTests.swift
git commit -m "ios: add Swift Testing unit-test target with smoke test"
```

(Note: the generated `AlephPaneliOS.xcodeproj/` is gitignored — do not commit it.)

---

### Task 2: `PairingTarget` + parse

Pure value type. Highest-value unit. TDD.

**Files:**
- Create: `mobile/ios/AlephPaneliOS/Models/PairingTarget.swift`
- Test: `mobile/ios/AlephPaneliOSTests/PairingTargetTests.swift`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `struct PairingTarget: Equatable { let url: URL }`
  - `enum PairingError: Error, Equatable { case empty, invalidURL, unsupportedScheme(String), noHost }`
  - `static func PairingTarget.parse(_ raw: String) -> Result<PairingTarget, PairingError>`
  - `static let PairingTarget.defaultPort: UInt16 = 18790`
  - `var PairingTarget.host: String` and `var PairingTarget.port: UInt16` (derived from `url`).

- [ ] **Step 1: Write the failing tests**

Create `mobile/ios/AlephPaneliOSTests/PairingTargetTests.swift`:

```swift
import Testing
import Foundation
@testable import AlephPaneliOS

@Suite struct PairingTargetTests {
    @Test("empty and whitespace reject")
    func emptyRejected() {
        #expect(PairingTarget.parse("") == .failure(.empty))
        #expect(PairingTarget.parse("   ") == .failure(.empty))
    }

    @Test("bare host gets http and default port")
    func bareHost() throws {
        let t = try PairingTarget.parse("192.168.1.5").get()
        #expect(t.url.absoluteString == "http://192.168.1.5:18790")
        #expect(t.host == "192.168.1.5")
        #expect(t.port == 18790)
    }

    @Test("host:port keeps user port, adds http")
    func hostPort() throws {
        let t = try PairingTarget.parse("box.lan:9000").get()
        #expect(t.url.absoluteString == "http://box.lan:9000")
        #expect(t.port == 9000)
    }

    @Test("explicit scheme preserved, default port added")
    func explicitScheme() throws {
        let t = try PairingTarget.parse("https://gw.example.com").get()
        #expect(t.url.absoluteString == "https://gw.example.com:18790")
    }

    @Test("explicit port preserved with scheme")
    func explicitPort() throws {
        let t = try PairingTarget.parse("https://gw.example.com:443").get()
        #expect(t.port == 443)
    }

    @Test("token query is preserved")
    func tokenPreserved() throws {
        let t = try PairingTarget.parse("http://127.0.0.1:18790/?token=aleph-abc123").get()
        #expect(t.url.query?.contains("token=aleph-abc123") == true)
        #expect(t.port == 18790)
    }

    @Test("unsupported schemes rejected")
    func unsupportedScheme() {
        #expect(PairingTarget.parse("ftp://host") == .failure(.unsupportedScheme("ftp")))
        #expect(PairingTarget.parse("ws://host") == .failure(.unsupportedScheme("ws")))
    }

    @Test("ipv6 with and without port")
    func ipv6() throws {
        let withPort = try PairingTarget.parse("http://[::1]:9000").get()
        #expect(withPort.port == 9000)
        let noPort = try PairingTarget.parse("http://[::1]").get()
        #expect(noPort.port == 18790)
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `xcodebuild test -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -destination 'platform=iOS Simulator,name=iPhone 16' -only-testing:AlephPaneliOSTests/PairingTargetTests`
Expected: FAIL — `Cannot find 'PairingTarget' in scope` (compile error).

- [ ] **Step 3: Implement `PairingTarget`**

Create `mobile/ios/AlephPaneliOS/Models/PairingTarget.swift`:

```swift
import Foundation

/// A validated connection target: the full URL of an `aleph-server` Gateway,
/// including any `?token=…`. Parsing mirrors the desktop lite shell's
/// `ConnectionTarget::parse` (default scheme http, default port 18790) so the
/// two shells share one onboarding format. iOS has no Local variant — the phone
/// shell never embeds a server.
struct PairingTarget: Equatable {
    let url: URL

    static let defaultPort: UInt16 = 18790

    /// Host of the target (without brackets for IPv6).
    var host: String {
        URLComponents(url: url, resolvingAgainstBaseURL: false)?.host ?? ""
    }

    /// Port of the target; falls back to `defaultPort` if somehow absent.
    var port: UInt16 {
        guard let p = URLComponents(url: url, resolvingAgainstBaseURL: false)?.port else {
            return Self.defaultPort
        }
        return UInt16(p)
    }

    /// Parse user/raw input into a target. See `PairingError` for rejections.
    static func parse(_ raw: String) -> Result<PairingTarget, PairingError> {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return .failure(.empty) }

        let withScheme = trimmed.contains("://") ? trimmed : "http://\(trimmed)"
        guard var components = URLComponents(string: withScheme) else {
            return .failure(.invalidURL)
        }
        switch components.scheme {
        case "http", "https":
            break
        case let other?:
            return .failure(.unsupportedScheme(other))
        case nil:
            return .failure(.invalidURL)
        }
        guard let host = components.host, !host.isEmpty else {
            return .failure(.noHost)
        }
        // URLComponents.port is non-nil only when the user wrote an explicit
        // port (it does NOT apply scheme defaults), so this cleanly injects the
        // default only when none was supplied.
        if components.port == nil {
            components.port = Int(Self.defaultPort)
        }
        guard let url = components.url else { return .failure(.invalidURL) }
        return .success(PairingTarget(url: url))
    }
}

enum PairingError: Error, Equatable {
    case empty
    case invalidURL
    case unsupportedScheme(String)
    case noHost
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `xcodebuild test -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -destination 'platform=iOS Simulator,name=iPhone 16' -only-testing:AlephPaneliOSTests/PairingTargetTests`
Expected: PASS — all 8 tests green.

- [ ] **Step 5: Commit**

```bash
git add mobile/ios/AlephPaneliOS/Models/PairingTarget.swift mobile/ios/AlephPaneliOSTests/PairingTargetTests.swift
git commit -m "ios: add PairingTarget with desktop-parity address parsing"
```

---

### Task 3: `ReachabilityProbe`

Bare TCP connect probe via `NWConnection`. TDD.

**Files:**
- Create: `mobile/ios/AlephPaneliOS/Services/ReachabilityProbe.swift`
- Test: `mobile/ios/AlephPaneliOSTests/ReachabilityProbeTests.swift`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `protocol ReachabilityProbing { func probe(host: String, port: UInt16) async -> Bool }`
  - `struct NWReachabilityProbe: ReachabilityProbing { init(timeout: TimeInterval = 2.0) }`

- [ ] **Step 1: Write the failing tests**

Create `mobile/ios/AlephPaneliOSTests/ReachabilityProbeTests.swift`:

```swift
import Testing
import Network
@testable import AlephPaneliOS

@Suite struct ReachabilityProbeTests {
    enum ProbeTestError: Error { case noPort }

    @Test("closed port probes false")
    func closedPortFalse() async {
        let probe = NWReachabilityProbe(timeout: 0.3)
        let ok = await probe.probe(host: "127.0.0.1", port: 1)
        #expect(ok == false)
    }

    @Test("open port probes true")
    func openPortTrue() async throws {
        let listener = try NWListener(using: .tcp)
        listener.newConnectionHandler = { $0.cancel() }
        let port: UInt16 = try await withCheckedThrowingContinuation { cont in
            listener.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    if let p = listener.port?.rawValue {
                        cont.resume(returning: p)
                    } else {
                        cont.resume(throwing: ProbeTestError.noPort)
                    }
                case .failed(let e):
                    cont.resume(throwing: e)
                default:
                    break
                }
            }
            listener.start(queue: .global())
        }
        let probe = NWReachabilityProbe(timeout: 1.0)
        let ok = await probe.probe(host: "127.0.0.1", port: port)
        listener.cancel()
        #expect(ok == true)
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `xcodebuild test -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -destination 'platform=iOS Simulator,name=iPhone 16' -only-testing:AlephPaneliOSTests/ReachabilityProbeTests`
Expected: FAIL — `Cannot find 'NWReachabilityProbe' in scope`.

- [ ] **Step 3: Implement the probe**

Create `mobile/ios/AlephPaneliOS/Services/ReachabilityProbe.swift`:

```swift
import Foundation
import Network

/// Whether a Gateway endpoint is currently accepting TCP connections. True
/// reachability, auth, and TLS are the webview's concern; this only answers
/// "is the port open" before we commit a navigation — mirrors the desktop lite
/// shell's pre-navigation probe (`connect_setup.rs::probe_reachable`).
protocol ReachabilityProbing {
    func probe(host: String, port: UInt16) async -> Bool
}

struct NWReachabilityProbe: ReachabilityProbing {
    let timeout: TimeInterval

    init(timeout: TimeInterval = 2.0) {
        self.timeout = timeout
    }

    func probe(host: String, port: UInt16) async -> Bool {
        guard let nwPort = NWEndpoint.Port(rawValue: port) else { return false }
        let connection = NWConnection(host: NWEndpoint.Host(host), port: nwPort, using: .tcp)
        let queue = DispatchQueue(label: "ai.aleph.panel.probe")

        return await withCheckedContinuation { continuation in
            // A small actor-free latch so we resume exactly once whether the
            // connection becomes ready, fails, or the timeout fires first.
            let resumed = ResumeOnce(continuation: continuation) {
                connection.cancel()
            }

            connection.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    resumed.fire(true)
                case .failed, .cancelled:
                    resumed.fire(false)
                default:
                    break
                }
            }
            connection.start(queue: queue)
            queue.asyncAfter(deadline: .now() + timeout) {
                resumed.fire(false)
            }
        }
    }
}

/// Resumes a continuation at most once and cancels the connection on first fire.
private final class ResumeOnce {
    private var done = false
    private let lock = NSLock()
    private let continuation: CheckedContinuation<Bool, Never>
    private let onResume: () -> Void

    init(continuation: CheckedContinuation<Bool, Never>, onResume: @escaping () -> Void) {
        self.continuation = continuation
        self.onResume = onResume
    }

    func fire(_ value: Bool) {
        lock.lock()
        defer { lock.unlock() }
        guard !done else { return }
        done = true
        onResume()
        continuation.resume(returning: value)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `xcodebuild test -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -destination 'platform=iOS Simulator,name=iPhone 16' -only-testing:AlephPaneliOSTests/ReachabilityProbeTests`
Expected: PASS — both tests green.

- [ ] **Step 5: Commit**

```bash
git add mobile/ios/AlephPaneliOS/Services/ReachabilityProbe.swift mobile/ios/AlephPaneliOSTests/ReachabilityProbeTests.swift
git commit -m "ios: add NWConnection TCP reachability probe"
```

---

### Task 4: `ConnectionStore` (Keychain) + in-memory fake

Persistence behind a protocol. TDD round-trip on the simulator keychain.

**Files:**
- Create: `mobile/ios/AlephPaneliOS/Services/ConnectionStore.swift`
- Create: `mobile/ios/AlephPaneliOSTests/InMemoryConnectionStore.swift` (test-only fake, reused by Task 5)
- Test: `mobile/ios/AlephPaneliOSTests/KeychainConnectionStoreTests.swift`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `protocol ConnectionStoring { func load() -> URL?; func save(_ url: URL) throws; func clear() }`
  - `struct KeychainConnectionStore: ConnectionStoring { init() }`
  - test-only `final class InMemoryConnectionStore: ConnectionStoring { init(_ initial: URL?) }`

- [ ] **Step 1: Write the failing tests**

Create `mobile/ios/AlephPaneliOSTests/KeychainConnectionStoreTests.swift`:

```swift
import Testing
import Foundation
@testable import AlephPaneliOS

@Suite struct KeychainConnectionStoreTests {
    let store = KeychainConnectionStore()

    init() { store.clear() } // isolate each test from prior keychain state

    @Test("save then load round-trips the full URL")
    func roundTrip() throws {
        let url = URL(string: "http://127.0.0.1:18790/?token=aleph-xyz")!
        try store.save(url)
        #expect(store.load() == url)
    }

    @Test("save overwrites a previous value")
    func overwrite() throws {
        try store.save(URL(string: "http://a.lan:18790")!)
        try store.save(URL(string: "http://b.lan:9000")!)
        #expect(store.load() == URL(string: "http://b.lan:9000")!)
    }

    @Test("clear removes the value")
    func clearRemoves() throws {
        try store.save(URL(string: "http://a.lan:18790")!)
        store.clear()
        #expect(store.load() == nil)
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `xcodebuild test -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -destination 'platform=iOS Simulator,name=iPhone 16' -only-testing:AlephPaneliOSTests/KeychainConnectionStoreTests`
Expected: FAIL — `Cannot find 'KeychainConnectionStore' in scope`.

- [ ] **Step 3: Implement `ConnectionStore`**

Create `mobile/ios/AlephPaneliOS/Services/ConnectionStore.swift`:

```swift
import Foundation
import Security

/// Persists the chosen connection target (full Gateway URL, including its
/// `?token=` secret). The target is transport config — R2/R4 keep all business
/// state in the WASM panel.
protocol ConnectionStoring {
    func load() -> URL?
    func save(_ url: URL) throws
    func clear()
}

enum KeychainError: Error {
    case unexpectedStatus(OSStatus)
}

/// Keychain-backed store. The URL carries the token, so it lives in the
/// Keychain (never UserDefaults) per the Swift security rules.
struct KeychainConnectionStore: ConnectionStoring {
    private let service = "ai.aleph.panel"
    private let account = "pairing-url"

    private var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    func load() -> URL? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess,
              let data = item as? Data,
              let string = String(data: data, encoding: .utf8),
              let url = URL(string: string) else {
            return nil
        }
        return url
    }

    func save(_ url: URL) throws {
        let data = Data(url.absoluteString.utf8)
        // Try update first; if nothing to update, add.
        let updateStatus = SecItemUpdate(
            baseQuery as CFDictionary,
            [kSecValueData as String: data] as CFDictionary
        )
        if updateStatus == errSecSuccess { return }
        if updateStatus == errSecItemNotFound {
            var addQuery = baseQuery
            addQuery[kSecValueData as String] = data
            addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
            let addStatus = SecItemAdd(addQuery as CFDictionary, nil)
            guard addStatus == errSecSuccess else {
                throw KeychainError.unexpectedStatus(addStatus)
            }
            return
        }
        throw KeychainError.unexpectedStatus(updateStatus)
    }

    func clear() {
        SecItemDelete(baseQuery as CFDictionary)
    }
}
```

- [ ] **Step 4: Add the in-memory fake (reused in Task 5)**

Create `mobile/ios/AlephPaneliOSTests/InMemoryConnectionStore.swift`:

```swift
import Foundation
@testable import AlephPaneliOS

final class InMemoryConnectionStore: ConnectionStoring {
    private var stored: URL?
    init(_ initial: URL? = nil) { stored = initial }
    func load() -> URL? { stored }
    func save(_ url: URL) throws { stored = url }
    func clear() { stored = nil }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `xcodebuild test -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -destination 'platform=iOS Simulator,name=iPhone 16' -only-testing:AlephPaneliOSTests/KeychainConnectionStoreTests`
Expected: PASS — round-trip, overwrite, clear all green.

- [ ] **Step 6: Commit**

```bash
git add mobile/ios/AlephPaneliOS/Services/ConnectionStore.swift mobile/ios/AlephPaneliOSTests/InMemoryConnectionStore.swift mobile/ios/AlephPaneliOSTests/KeychainConnectionStoreTests.swift
git commit -m "ios: add Keychain-backed connection store with in-memory test fake"
```

---

### Task 5: `AppState` resolution / submit / reconfigure

The brain that decides which screen to show. TDD with injected fakes.

**Files:**
- Create: `mobile/ios/AlephPaneliOS/State/AppState.swift`
- Test: `mobile/ios/AlephPaneliOSTests/AppStateTests.swift`

**Interfaces:**
- Consumes: `PairingTarget` (Task 2), `ReachabilityProbing` (Task 3), `ConnectionStoring` + `InMemoryConnectionStore` (Task 4).
- Produces:
  - `@MainActor final class AppState: ObservableObject`
  - `enum AppState.Screen: Equatable { case pairing(message: String?); case connected(URL) }`
  - `@Published private(set) var screen: Screen`
  - `init(store: ConnectionStoring, probe: ReachabilityProbing, envURL: @escaping () -> String? = …)`
  - `func resolve() async`
  - `func submit(_ raw: String) async`
  - `func requestReconfigure(message: String? = nil)`
  - `func currentTargetString() -> String`

- [ ] **Step 1: Write the failing tests**

Create `mobile/ios/AlephPaneliOSTests/AppStateTests.swift`:

```swift
import Testing
import Foundation
@testable import AlephPaneliOS

private struct StubProbe: ReachabilityProbing {
    let reachable: Bool
    func probe(host: String, port: UInt16) async -> Bool { reachable }
}

@MainActor
@Suite struct AppStateTests {
    @Test("env URL wins and is persisted")
    func envWins() async {
        let store = InMemoryConnectionStore()
        let state = AppState(store: store, probe: StubProbe(reachable: true),
                             envURL: { "http://127.0.0.1:18790/?token=aleph-env" })
        await state.resolve()
        #expect(state.screen == .connected(URL(string: "http://127.0.0.1:18790/?token=aleph-env")!))
        #expect(store.load() == URL(string: "http://127.0.0.1:18790/?token=aleph-env")!)
    }

    @Test("saved + reachable connects")
    func savedReachable() async {
        let store = InMemoryConnectionStore(URL(string: "http://box.lan:9000")!)
        let state = AppState(store: store, probe: StubProbe(reachable: true), envURL: { nil })
        await state.resolve()
        #expect(state.screen == .connected(URL(string: "http://box.lan:9000")!))
    }

    @Test("saved + unreachable falls to pairing with message")
    func savedUnreachable() async {
        let store = InMemoryConnectionStore(URL(string: "http://box.lan:9000")!)
        let state = AppState(store: store, probe: StubProbe(reachable: false), envURL: { nil })
        await state.resolve()
        #expect(state.screen == .pairing(message: "Last server unreachable"))
    }

    @Test("no env, empty store → pairing(nil)")
    func emptyStore() async {
        let state = AppState(store: InMemoryConnectionStore(), probe: StubProbe(reachable: true), envURL: { nil })
        await state.resolve()
        #expect(state.screen == .pairing(message: nil))
    }

    @Test("submit valid + reachable connects and persists")
    func submitReachable() async {
        let store = InMemoryConnectionStore()
        let state = AppState(store: store, probe: StubProbe(reachable: true), envURL: { nil })
        await state.submit("192.168.1.5")
        #expect(state.screen == .connected(URL(string: "http://192.168.1.5:18790")!))
        #expect(store.load() == URL(string: "http://192.168.1.5:18790")!)
    }

    @Test("submit invalid stays on pairing with message")
    func submitInvalid() async {
        let state = AppState(store: InMemoryConnectionStore(), probe: StubProbe(reachable: true), envURL: { nil })
        await state.submit("")
        if case .pairing(let message) = state.screen {
            #expect(message != nil)
        } else {
            Issue.record("expected pairing screen")
        }
    }

    @Test("submit valid + unreachable shows not-reachable")
    func submitUnreachable() async {
        let state = AppState(store: InMemoryConnectionStore(), probe: StubProbe(reachable: false), envURL: { nil })
        await state.submit("box.lan:9000")
        #expect(state.screen == .pairing(message: "box.lan:9000 is not reachable"))
    }

    @Test("requestReconfigure switches to pairing")
    func reconfigure() async {
        let state = AppState(store: InMemoryConnectionStore(URL(string: "http://a.lan:18790")!),
                             probe: StubProbe(reachable: true), envURL: { nil })
        await state.resolve()
        state.requestReconfigure()
        #expect(state.screen == .pairing(message: nil))
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `xcodebuild test -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -destination 'platform=iOS Simulator,name=iPhone 16' -only-testing:AlephPaneliOSTests/AppStateTests`
Expected: FAIL — `Cannot find 'AppState' in scope`.

- [ ] **Step 3: Implement `AppState`**

Create `mobile/ios/AlephPaneliOS/State/AppState.swift`:

```swift
import Foundation

/// Drives which screen the shell shows: the native pairing screen (transport
/// config only) or the WASM panel. Holds no business state — that all lives in
/// the panel (R2/R4).
@MainActor
final class AppState: ObservableObject {
    enum Screen: Equatable {
        case pairing(message: String?)
        case connected(URL)
    }

    @Published private(set) var screen: Screen = .pairing(message: nil)

    private let store: ConnectionStoring
    private let probe: ReachabilityProbing
    private let envURL: () -> String?

    init(
        store: ConnectionStoring,
        probe: ReachabilityProbing,
        envURL: @escaping () -> String? = { ProcessInfo.processInfo.environment["PANEL_URL"] }
    ) {
        self.store = store
        self.probe = probe
        self.envURL = envURL
    }

    /// Startup resolution: env wins (dev/sim injection), then the persisted
    /// target, else the pairing screen. A persisted/env target is probed before
    /// navigating — unreachable falls back to pairing instead of a dead webview.
    func resolve() async {
        if let env = envURL(), !env.isEmpty,
           case .success(let target) = PairingTarget.parse(env) {
            try? store.save(target.url)
            await connectOrPair(target)
            return
        }
        if let saved = store.load() {
            await connectOrPair(PairingTarget(url: saved))
            return
        }
        screen = .pairing(message: nil)
    }

    /// Validate + probe a user-entered address; persist + connect on success,
    /// otherwise stay on the pairing screen with an inline message.
    func submit(_ raw: String) async {
        switch PairingTarget.parse(raw) {
        case .failure(let error):
            screen = .pairing(message: Self.message(for: error))
        case .success(let target):
            if await probe.probe(host: target.host, port: target.port) {
                try? store.save(target.url)
                screen = .connected(target.url)
            } else {
                screen = .pairing(message: "\(target.host):\(target.port) is not reachable")
            }
        }
    }

    /// Reveal the pairing screen on demand (shake gesture / webview load failure).
    func requestReconfigure(message: String? = nil) {
        screen = .pairing(message: message)
    }

    /// Current persisted target as a prefill string for the pairing field.
    func currentTargetString() -> String {
        store.load()?.absoluteString ?? ""
    }

    private func connectOrPair(_ target: PairingTarget) async {
        if await probe.probe(host: target.host, port: target.port) {
            screen = .connected(target.url)
        } else {
            screen = .pairing(message: "Last server unreachable")
        }
    }

    private static func message(for error: PairingError) -> String {
        switch error {
        case .empty: return "Enter a server address"
        case .invalidURL: return "That doesn't look like a valid address"
        case .unsupportedScheme(let s): return "Unsupported scheme: \(s)"
        case .noHost: return "Address is missing a host"
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `xcodebuild test -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -destination 'platform=iOS Simulator,name=iPhone 16' -only-testing:AlephPaneliOSTests/AppStateTests`
Expected: PASS — all 8 tests green.

- [ ] **Step 5: Commit**

```bash
git add mobile/ios/AlephPaneliOS/State/AppState.swift mobile/ios/AlephPaneliOSTests/AppStateTests.swift
git commit -m "ios: add AppState screen resolution with probe-gated connect"
```

---

### Task 6: Pairing UI + shake + webview delegate + wiring (integration)

Presentation + wiring. No unit tests (UI); verified by build + the manual QA in Step 7. Folds in the README update.

**Files:**
- Create: `mobile/ios/AlephPaneliOS/Views/PairingView.swift`
- Create: `mobile/ios/AlephPaneliOS/Views/ShakeDetector.swift`
- Modify: `mobile/ios/AlephPaneliOS/Views/PanelWebView.swift` (add navigation delegate)
- Modify: `mobile/ios/AlephPaneliOS/Views/ContentView.swift` (switch on `AppState.screen`)
- Modify: `mobile/ios/AlephPaneliOS/App/AlephPaneliOSApp.swift` (inject `AppState`)
- Modify: `mobile/ios/README.md`

**Interfaces:**
- Consumes: `AppState` (Task 5), `PanelWebView` (existing).
- Produces: a fully wired app — first run → pairing; connect → panel; shake / load-failure → pairing.

- [ ] **Step 1: Implement `PairingView`**

Create `mobile/ios/AlephPaneliOS/Views/PairingView.swift`:

```swift
import SwiftUI

/// Native first-run / reconfigure screen. Transport config ONLY (which server
/// to connect to) — all app UI lives in the WASM panel (R2/R4). Mirrors the
/// desktop lite shell's `connect.html` manual-entry card.
struct PairingView: View {
    @EnvironmentObject private var appState: AppState

    let initialText: String
    let message: String?

    @State private var address: String
    @State private var submitting = false

    init(initialText: String, message: String?) {
        self.initialText = initialText
        self.message = message
        _address = State(initialValue: initialText)
    }

    var body: some View {
        VStack(spacing: 16) {
            Text("Connect to Aleph")
                .font(.title2).bold()
            Text("Enter your Aleph server address — e.g. 192.168.1.5 or http://gw.example.com")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)

            TextField("host, host:port, or http(s)://host", text: $address)
                .textFieldStyle(.roundedBorder)
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled(true)
                .submitLabel(.go)
                .onSubmit(connect)

            Button(action: connect) {
                Text(submitting ? "Connecting…" : "Connect")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .disabled(submitting || address.trimmingCharacters(in: .whitespaces).isEmpty)

            if let message {
                Text(message)
                    .font(.footnote)
                    .foregroundStyle(.red)
                    .multilineTextAlignment(.center)
            }
        }
        .padding(28)
        .frame(maxWidth: 420)
    }

    private func connect() {
        guard !submitting else { return }
        submitting = true
        Task {
            await appState.submit(address)
            submitting = false
        }
    }
}
```

- [ ] **Step 2: Implement `ShakeDetector`**

Create `mobile/ios/AlephPaneliOS/Views/ShakeDetector.swift`:

```swift
import SwiftUI
import UIKit

/// Bridges the device shake motion to a closure. Hosted as a hidden background
/// view so the shell can reveal the pairing screen on shake without any
/// panel-side coupling.
struct ShakeDetector: UIViewControllerRepresentable {
    let onShake: () -> Void

    func makeUIViewController(context: Context) -> ShakeViewController {
        let vc = ShakeViewController()
        vc.onShake = onShake
        return vc
    }

    func updateUIViewController(_ vc: ShakeViewController, context: Context) {
        vc.onShake = onShake
    }
}

final class ShakeViewController: UIViewController {
    var onShake: (() -> Void)?

    override var canBecomeFirstResponder: Bool { true }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        becomeFirstResponder()
    }

    override func motionEnded(_ motion: UIEvent.EventSubtype, with event: UIEvent?) {
        if motion == .motionShake { onShake?() }
    }
}
```

- [ ] **Step 3: Add the navigation delegate to `PanelWebView`**

Replace the contents of `mobile/ios/AlephPaneliOS/Views/PanelWebView.swift` with (keeps the existing viewport-cover injection; adds a coordinator that reports load failures):

```swift
import SwiftUI
import WebKit

/// Full-screen `WKWebView` hosting the Aleph WASM panel.
struct PanelWebView: UIViewRepresentable {
    let url: URL
    var onLoadFailure: (String) -> Void = { _ in }

    func makeCoordinator() -> Coordinator {
        Coordinator(onLoadFailure: onLoadFailure)
    }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()

        // The panel's static viewport meta omits `viewport-fit=cover`, so iOS
        // reports zero safe-area insets and the phone shell's
        // `env(safe-area-inset-*)` padding collapses. Rewrite the meta at
        // document end so the insets resolve correctly.
        let coverJS = """
        (function () {
          var m = document.querySelector('meta[name=viewport]');
          var v = 'width=device-width, initial-scale=1, viewport-fit=cover';
          if (m) { m.setAttribute('content', v); }
          else {
            m = document.createElement('meta');
            m.name = 'viewport'; m.content = v;
            document.head.appendChild(m);
          }
        })();
        """
        config.userContentController.addUserScript(
            WKUserScript(source: coverJS, injectionTime: .atDocumentEnd, forMainFrameOnly: true)
        )
        config.allowsInlineMediaPlayback = true

        let webView = WKWebView(frame: .zero, configuration: config)
        webView.navigationDelegate = context.coordinator
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        webView.scrollView.bounces = false
        webView.isInspectable = true
        webView.load(URLRequest(url: url))
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {}

    final class Coordinator: NSObject, WKNavigationDelegate {
        let onLoadFailure: (String) -> Void

        init(onLoadFailure: @escaping (String) -> Void) {
            self.onLoadFailure = onLoadFailure
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            onLoadFailure(error.localizedDescription)
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            onLoadFailure(error.localizedDescription)
        }
    }
}
```

- [ ] **Step 4: Rewrite `ContentView` to switch on `AppState`**

Replace the contents of `mobile/ios/AlephPaneliOS/Views/ContentView.swift` with:

```swift
import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var appState: AppState

    var body: some View {
        Group {
            switch appState.screen {
            case .pairing(let message):
                PairingView(initialText: appState.currentTargetString(), message: message)
            case .connected(let url):
                PanelWebView(url: url, onLoadFailure: { appState.requestReconfigure(message: $0) })
                    .ignoresSafeArea()
            }
        }
        .background(ShakeDetector { appState.requestReconfigure() })
        .task { await appState.resolve() }
    }
}
```

- [ ] **Step 5: Inject `AppState` in the app entry point**

Replace the contents of `mobile/ios/AlephPaneliOS/App/AlephPaneliOSApp.swift` with:

```swift
import SwiftUI

/// Thin native shell for the Aleph phone panel. A full-screen `WKWebView` over
/// the WASM panel served by an `aleph-server`; the native layer only handles
/// transport config (which server to connect to). See R2/R6 in the root CLAUDE.md.
@main
struct AlephPaneliOSApp: App {
    @StateObject private var appState = AppState(
        store: KeychainConnectionStore(),
        probe: NWReachabilityProbe()
    )

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(appState)
        }
    }
}
```

- [ ] **Step 6: Update the README**

In `mobile/ios/README.md`, replace the "No secrets in source" section's connection description with a short note that the app now shows a native pairing screen on first launch. Add this block after the title paragraph:

```markdown
## Connecting

On first launch the app shows a native **pairing screen**: enter your Aleph
server address (`host`, `host:port`, or `http(s)://host[/route]?token=…`). The
target is probed for reachability, then stored in the **Keychain** (the token
never touches `UserDefaults`). To re-configure later, **shake the device** to
return to the pairing screen; an unreachable server also falls back to it
automatically.

The `PANEL_URL` launch env still takes priority (used by `generate.sh` /
`launch-local.sh` for sim/dev), so existing test flows are unchanged.
```

- [ ] **Step 7: Regenerate, build, and manual-QA**

Regenerate + build:
Run: `xcodegen generate && xcodebuild build -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -destination 'platform=iOS Simulator,name=iPhone 16'`
Expected: `** BUILD SUCCEEDED **`.

Manual QA (per the project's iOS test protocol — rebuild the full macOS app so its embedded core serves the current dist at `:18790`, then point the sim at it). Verify:
1. Fresh install (`xcrun simctl uninstall booted ai.aleph.panel.iossim` first) → launches into the **pairing screen** (not blank).
2. Enter `127.0.0.1:18790/?token=<token>` → connects, panel renders.
3. **Shake** (Simulator menu: Device ▸ Shake, or `⌃⌘Z`) → returns to pairing, address prefilled.
4. Enter an unreachable address → inline error, no navigation, no blank screen.
5. Kill the local core → webview load fails → falls back to pairing.
6. Relaunch → Keychain remembers the last server, connects without re-entry.

- [ ] **Step 8: Commit**

```bash
git add mobile/ios/AlephPaneliOS/Views/PairingView.swift mobile/ios/AlephPaneliOS/Views/ShakeDetector.swift mobile/ios/AlephPaneliOS/Views/PanelWebView.swift mobile/ios/AlephPaneliOS/Views/ContentView.swift mobile/ios/AlephPaneliOS/App/AlephPaneliOSApp.swift mobile/ios/README.md
git commit -m "ios: native pairing screen, shake-to-reconfigure, load-failure fallback"
```

---

### Task 7: Bundle ID + VERSION wiring

Orthogonal project config. No product-logic change.

**Files:**
- Modify: `mobile/ios/project.yml`
- Modify: `mobile/ios/generate.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: generated app with Bundle ID `ai.aleph.panel` and version strings from the repo `VERSION` file.

- [ ] **Step 1: Update Bundle ID + version properties in `project.yml`**

In `mobile/ios/project.yml`, under `targets.AlephPaneliOS.settings.base`, change:

```yaml
        PRODUCT_BUNDLE_IDENTIFIER: ai.aleph.panel
```

(was `ai.aleph.panel.iossim`).

And under `targets.AlephPaneliOS.info.properties`, change the two version lines to env-substituted values:

```yaml
        CFBundleShortVersionString: ${ALEPH_VERSION}
        CFBundleVersion: ${ALEPH_VERSION}
```

(was `"0.1"` / `"1"`). Leave every other property — including the `NSAppTransportSecurity` block — untouched.

- [ ] **Step 2: Export `ALEPH_VERSION` in `generate.sh`**

In `mobile/ios/generate.sh`, add this line immediately after `cd "$(dirname "$0")"` (before the `xcodegen generate` call):

```bash
# Version strings come from the repo's single VERSION source (CalVer), mirrored
# into the generated Info.plist the same way PANEL_URL is injected into the scheme.
export ALEPH_VERSION="$(cat ../../VERSION | tr -d '[:space:]')"
```

- [ ] **Step 3: Verify the version is injected (primary path)**

Run (dummy PANEL_URL avoids the local-core token fetch):
`PANEL_URL="http://127.0.0.1:18790/" ./generate.sh && plutil -p AlephPaneliOS/Resources/Info.plist | grep -E 'CFBundleShortVersionString|CFBundleVersion'`
Expected: both show the current VERSION (e.g. `"CFBundleShortVersionString" => "26.6.24"`).

**If the values still show `${ALEPH_VERSION}` (xcodegen did not expand env in `info.properties`) — fallback path:** instead of `${ALEPH_VERSION}` in `project.yml`, keep the literal `"0.1"`/`"1"` there and append to `generate.sh` AFTER `xcodegen generate`:

```bash
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $ALEPH_VERSION" AlephPaneliOS/Resources/Info.plist
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $ALEPH_VERSION" AlephPaneliOS/Resources/Info.plist
```

Re-run the verify command; expect the VERSION values.

- [ ] **Step 4: Confirm the Bundle ID changed**

Run: `xcodegen generate && xcodebuild -showBuildSettings -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS 2>/dev/null | grep PRODUCT_BUNDLE_IDENTIFIER | head -1`
Expected: `PRODUCT_BUNDLE_IDENTIFIER = ai.aleph.panel`.
Also run `grep -rn iossim mobile/ios --include='*.swift' --include='*.yml'` — expect only the test target's `ai.aleph.panel.iossim.tests` (no stray app references).

- [ ] **Step 5: Commit**

```bash
git add mobile/ios/project.yml mobile/ios/generate.sh mobile/ios/AlephPaneliOS/Resources/Info.plist
git commit -m "ios: set release Bundle ID and wire version to VERSION file"
```

---

## Self-Review

**Spec coverage (§ → task):**
- §2.1 PairingTarget → Task 2 ✓; ConnectionStore → Task 4 ✓; ReachabilityProbe → Task 3 ✓; AppState → Task 5 ✓; PairingView → Task 6 ✓; ShakeDetector → Task 6 ✓.
- §2.2 ContentView / PanelWebView / App wiring → Task 6 ✓; project.yml + generate.sh + README → Tasks 1/6/7 ✓.
- §3 data flow (env → keychain → probe; submit; reconfigure) → Task 5 ✓.
- §4 Keychain storage, no UserDefaults token, no migration → Task 4 + Task 6 ✓; ATS untouched → Global Constraints + Task 7 Step 1 ✓.
- §5 Bundle ID + VERSION (primary + PlistBuddy fallback) → Task 7 ✓.
- §6.1 unit tests (parse/probe/resolve) → Tasks 2/3/5 ✓; test target → Task 1 ✓.
- §6.2 runtime QA six steps → Task 6 Step 7 ✓.
- §8 non-goals (no iPad, no QR/Bonjour, no panel button, no ATS) → honored; none introduced.
- §9 acceptance criteria → covered across Tasks 2–7.

**Placeholder scan:** no TBD/TODO/"handle edge cases"/"similar to" — all steps carry concrete code/commands. ✓

**Type consistency:** `ConnectionStoring`/`ReachabilityProbing` protocol names, `PairingTarget.parse → Result<PairingTarget, PairingError>`, `AppState.Screen.{pairing(message:),connected(URL)}`, `currentTargetString()`, `onLoadFailure:` — all referenced names match their producing task. ✓

**Known nuance to watch during execution:** Step destinations use `iPhone 16` — substitute a simulator that actually exists (`xcrun simctl list devices available`). The keychain round-trip test (Task 4) relies on the simulator keychain being writable from a hosted test bundle; if `SecItemAdd` returns `errSecMissingEntitlement`, add a minimal keychain-sharing entitlement to the test host (note, not expected on simulator).
