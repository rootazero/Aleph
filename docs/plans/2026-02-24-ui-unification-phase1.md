# UI Unification Phase 1: Settings WebView Integration

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace native settings UI with WebView loading Leptos Control Plane, achieving single-source settings across all platforms.

**Architecture:** macOS App gets a new NSWindow hosting WKWebView that loads `http://127.0.0.1:18790/settings`. Tauri App's settings window URL changes from local HTML to the same server URL. Zero new Leptos code needed — Control Plane already has 19 complete settings pages.

**Tech Stack:** Swift/AppKit (WKWebView), Tauri (webview URL config), Leptos/WASM (existing Control Plane)

**Design Doc:** `docs/plans/2026-02-24-ui-unification-design.md`

---

### Task 1: Add WebKit framework to macOS project

**Files:**
- Modify: `apps/macos/project.yml`

**Step 1: Add WebKit to framework dependencies**

In `apps/macos/project.yml`, add WebKit as a system framework to the Aleph target's dependencies (after line 127):

```yaml
    dependencies:
      - framework: Aleph/Frameworks/libalephcore.dylib
        embed: true
        link: true
      - package: GRDB
      - sdk: WebKit.framework
```

**Step 2: Regenerate Xcode project**

Run: `cd apps/macos && xcodegen generate`
Expected: Project regenerated with WebKit framework linked.

**Step 3: Verify build**

Run: `cd apps/macos && xcodebuild build -scheme Aleph -configuration Debug -quiet 2>&1 | tail -5`
Expected: BUILD SUCCEEDED

**Step 4: Commit**

```bash
git add apps/macos/project.yml
git commit -m "build(macos): add WebKit framework dependency for Settings WebView"
```

---

### Task 2: Create SettingsWebView (WKWebView wrapper)

**Files:**
- Create: `apps/macos/Aleph/Sources/Components/SettingsWebView.swift`

**Step 1: Write the WebView wrapper**

This is an NSViewRepresentable that wraps WKWebView with error handling for when the server isn't running.

```swift
//
//  SettingsWebView.swift
//  Aleph
//
//  WebView wrapper for loading Control Plane settings UI.
//

import SwiftUI
import WebKit

/// WebView that loads the Leptos Control Plane settings UI from the local server.
///
/// Handles:
/// - Loading the settings URL from localhost
/// - Server-not-running error detection
/// - Reload support
struct SettingsWebView: NSViewRepresentable {

    /// The URL to load in the WebView
    let url: URL

    /// Callback when the server is unreachable
    var onServerUnavailable: (() -> Void)?

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

        let webView = WKWebView(frame: .zero, configuration: config)
        webView.navigationDelegate = context.coordinator
        webView.isInspectable = true
        webView.load(URLRequest(url: url))
        return webView
    }

    func updateNSView(_ webView: WKWebView, context: Context) {
        // Only reload if URL changed
        if webView.url != url {
            webView.load(URLRequest(url: url))
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(onServerUnavailable: onServerUnavailable)
    }

    final class Coordinator: NSObject, WKNavigationDelegate, @unchecked Sendable {
        let onServerUnavailable: (() -> Void)?

        init(onServerUnavailable: (() -> Void)?) {
            self.onServerUnavailable = onServerUnavailable
        }

        nonisolated func webView(
            _ webView: WKWebView,
            didFailProvisionalNavigation navigation: WKNavigation!,
            withError error: Error
        ) {
            let nsError = error as NSError
            // NSURLErrorCannotConnectToHost or NSURLErrorTimedOut
            if nsError.domain == NSURLErrorDomain &&
               (nsError.code == NSURLErrorCannotConnectToHost ||
                nsError.code == NSURLErrorConnectionRefused ||
                nsError.code == NSURLErrorTimedOut) {
                Task { @MainActor in
                    onServerUnavailable?()
                }
            }
        }
    }
}
```

**Step 2: Verify syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py apps/macos/Aleph/Sources/Components/SettingsWebView.swift`
Expected: Syntax valid

**Step 3: Commit**

```bash
git add apps/macos/Aleph/Sources/Components/SettingsWebView.swift
git commit -m "feat(macos): add SettingsWebView WKWebView wrapper"
```

---

### Task 3: Create SettingsWindowController

**Files:**
- Create: `apps/macos/Aleph/Sources/Coordinator/SettingsWindowController.swift`

**Step 1: Write the window controller**

Follows the same patterns as `PermissionCoordinator.swift` (lines 73-104) for window creation.

```swift
//
//  SettingsWindowController.swift
//  Aleph
//
//  Manages the Settings window that hosts the Leptos Control Plane WebView.
//

import AppKit
import SwiftUI

/// Controller for the Settings window.
///
/// Creates an NSWindow hosting SettingsWebView pointed at the Control Plane.
/// Handles window lifecycle and server-unavailable errors.
@MainActor
final class SettingsWindowController {

    // MARK: - Properties

    /// The settings window (nil when not visible)
    private var window: NSWindow?

    /// Control Plane settings URL
    private let settingsURL = URL(string: "http://127.0.0.1:18790/settings")!

    // MARK: - Public API

    /// Show the settings window, creating it if needed.
    func showSettings() {
        if let existingWindow = window {
            existingWindow.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        let settingsView = SettingsWebView(
            url: settingsURL,
            onServerUnavailable: { [weak self] in
                self?.handleServerUnavailable()
            }
        )

        let hostingController = NSHostingController(rootView: settingsView)

        let newWindow = NSWindow(contentViewController: hostingController)
        newWindow.title = L("menu.settings")
        newWindow.setContentSize(NSSize(width: 900, height: 650))
        newWindow.minSize = NSSize(width: 700, height: 500)
        newWindow.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        newWindow.titlebarAppearsTransparent = false
        newWindow.center()
        newWindow.isReleasedWhenClosed = false
        newWindow.delegate = WindowCloseDelegate { [weak self] in
            self?.window = nil
        }

        window = newWindow
        newWindow.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    /// Close the settings window if open.
    func closeSettings() {
        window?.close()
        window = nil
    }

    // MARK: - Private

    private func handleServerUnavailable() {
        let alert = NSAlert()
        alert.messageText = "Aleph Server is not running"
        alert.informativeText = "The Control Plane is not available. Please start the Aleph Server first."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "OK")
        alert.runModal()

        closeSettings()
    }
}

// MARK: - Window Close Delegate

/// Lightweight delegate that fires a callback when the window closes.
private final class WindowCloseDelegate: NSObject, NSWindowDelegate, @unchecked Sendable {
    let onClose: () -> Void

    init(onClose: @escaping () -> Void) {
        self.onClose = onClose
    }

    func windowWillClose(_ notification: Notification) {
        Task { @MainActor in
            onClose()
        }
    }
}
```

**Step 2: Verify syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py apps/macos/Aleph/Sources/Coordinator/SettingsWindowController.swift`
Expected: Syntax valid

**Step 3: Commit**

```bash
git add apps/macos/Aleph/Sources/Coordinator/SettingsWindowController.swift
git commit -m "feat(macos): add SettingsWindowController for WebView settings"
```

---

### Task 4: Add Settings menu item to MenuBarManager

**Files:**
- Modify: `apps/macos/Aleph/Sources/Managers/MenuBarManager.swift`
- Modify: `apps/macos/Aleph/Sources/AppDelegate.swift`

**Step 1: Add showSettingsAction parameter to MenuBarManager.setup()**

In `MenuBarManager.swift`, update the `setup()` method signature (line 51) to accept a settings action:

```swift
    func setup(
        target: AnyObject,
        showAboutAction: Selector,
        showConversationAction: Selector,
        showSettingsAction: Selector,
        quitAction: Selector,
        debugActions: [(title: String, action: Selector, keyEquivalent: String)]? = nil
    ) {
```

**Step 2: Add the Settings menu item**

In `MenuBarManager.swift`, replace the comment at line 108 with an actual menu item. Insert after the provider submenu (after line 106):

```swift
        // Settings menu item — opens WebView with Control Plane
        let settingsItem = NSMenuItem(
            title: L("menu.settings"),
            action: showSettingsAction,
            keyEquivalent: ","
        )
        settingsItem.target = target
        menu.addItem(settingsItem)

        menu.addItem(NSMenuItem.separator())
```

Remove the old comment at line 108:
```
        // Settings menu item removed - all configuration now in Control Panel Dashboard
```

**Step 3: Update AppDelegate to pass the settings action**

In `AppDelegate.swift`, update `setupMenuBar()` (around line 271) to pass the new selector:

```swift
        menuBarManager?.setup(
            target: self,
            showAboutAction: #selector(showAbout),
            showConversationAction: #selector(showConversation),
            showSettingsAction: #selector(showSettings),
            quitAction: #selector(quit),
            debugActions: debugActions
        )
```

**Step 4: Add SettingsWindowController property and showSettings method to AppDelegate**

In `AppDelegate.swift`, add after line 29 (`haloWindow` property):

```swift
    // Settings window controller (WebView-based)
    private var settingsWindowController = SettingsWindowController()
```

Remove the comment at line 31:
```
    // Settings window removed - all configuration now in Control Panel Dashboard
```

Add the `showSettings()` method near the `showAbout()` method (after line 287):

```swift
    @objc private func showSettings() {
        settingsWindowController.showSettings()
    }
```

**Step 5: Verify syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py apps/macos/Aleph/Sources/Managers/MenuBarManager.swift`
Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py apps/macos/Aleph/Sources/AppDelegate.swift`
Expected: Both valid

**Step 6: Commit**

```bash
git add apps/macos/Aleph/Sources/Managers/MenuBarManager.swift apps/macos/Aleph/Sources/AppDelegate.swift
git commit -m "feat(macos): add Settings menu item opening Control Plane WebView"
```

---

### Task 5: Add localization strings for Settings menu

**Files:**
- Modify: `apps/macos/Aleph/Resources/en.lproj/Localizable.xcstrings` (or equivalent)
- Modify: `apps/macos/Aleph/Resources/zh-Hans.lproj/Localizable.xcstrings` (or equivalent)

**Step 1: Find localization files**

Run: `find apps/macos -name "*.xcstrings" -o -name "*.strings" | head -20`

**Step 2: Add "menu.settings" key**

English: `"Settings..."` or `"Settings"`
Chinese: `"设置..."` or `"设置"`

Note: The `L()` function already handles `"menu.settings"`. We need to ensure the key exists in localization files. If the key was previously removed when settings were deleted, re-add it.

**Step 3: Commit**

```bash
git add apps/macos/Aleph/Resources/
git commit -m "i18n(macos): add Settings menu localization strings"
```

---

### Task 6: Build and verify macOS changes

**Step 1: Regenerate Xcode project**

Run: `cd apps/macos && xcodegen generate`
Expected: Generated project with WebKit framework and new source files.

**Step 2: Build**

Run: `cd apps/macos && xcodebuild build -scheme Aleph -configuration Debug 2>&1 | tail -20`
Expected: BUILD SUCCEEDED

**Step 3: Manual verification checklist**

- [ ] Menu bar shows "Settings..." item with Cmd+, shortcut
- [ ] Clicking Settings opens a 900x650 window
- [ ] Window loads Control Plane UI from localhost:18790
- [ ] If server not running, shows alert dialog
- [ ] Window can be closed and reopened
- [ ] Window remembers nothing (fresh load each time) — this is fine for Phase 1

**Step 4: Commit any fixes**

```bash
git add -A && git commit -m "fix(macos): resolve build issues from Settings WebView integration"
```

---

### Task 7: Update Tauri settings window to load from server

**Files:**
- Modify: `apps/desktop/src-tauri/tauri.conf.json`

**Step 1: Change settings window URL**

In `tauri.conf.json`, update the settings window configuration (line 33) to load from the Aleph Server instead of local HTML:

Change:
```json
        "url": "/settings.html"
```
To:
```json
        "url": "http://127.0.0.1:18790/settings"
```

**Step 2: Update CSP to allow server connection**

In `tauri.conf.json`, update the CSP (line 45) to allow connections to the Control Plane server:

Change:
```json
      "csp": "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; img-src 'self' data: https:; connect-src 'self' https://*.openai.com https://*.anthropic.com https://*.googleapis.com"
```
To:
```json
      "csp": "default-src 'self' http://127.0.0.1:18790; style-src 'self' 'unsafe-inline' http://127.0.0.1:18790; script-src 'self' 'unsafe-eval' 'unsafe-inline' http://127.0.0.1:18790; img-src 'self' data: https: http://127.0.0.1:18790; connect-src 'self' http://127.0.0.1:18790 ws://127.0.0.1:18789 https://*.openai.com https://*.anthropic.com https://*.googleapis.com; frame-src http://127.0.0.1:18790"
```

Note: `'unsafe-eval'` is needed because Leptos WASM requires it for WebAssembly instantiation. `ws://127.0.0.1:18789` allows the embedded UI to connect to the Gateway WebSocket.

**Step 3: Verify Tauri build**

Run: `cd apps/desktop && cargo build --manifest-path src-tauri/Cargo.toml 2>&1 | tail -5`
Expected: Build succeeds (Rust backend doesn't depend on frontend content)

**Step 4: Commit**

```bash
git add apps/desktop/src-tauri/tauri.conf.json
git commit -m "feat(desktop): point settings window to Leptos Control Plane server"
```

---

### Task 8: Clean up stale comments and document the change

**Files:**
- Modify: `apps/macos/Aleph/Sources/Managers/MenuBarManager.swift` (remove stale comments)
- Modify: `apps/macos/Aleph/Sources/AppDelegate.swift` (remove stale comments)

**Step 1: Clean stale "removed" comments**

In `MenuBarManager.swift`:
- Remove lines 30-31: `// Settings functionality has been moved to Control Panel Dashboard`
- Remove line 48: `/// - showSettingsAction: Selector for "Settings" action - REMOVED`
- Remove line 181: `// setSettingsEnabled method removed - Settings menu no longer exists`

In `AppDelegate.swift`:
- Remove line 31: `// Settings window removed - all configuration now in Control Panel Dashboard`

Replace with accurate comments reflecting the new architecture:
- `// Settings window uses WebView loading Control Plane (Leptos/WASM)`

**Step 2: Verify syntax**

Run syntax verification on both files.

**Step 3: Commit**

```bash
git add apps/macos/Aleph/Sources/Managers/MenuBarManager.swift apps/macos/Aleph/Sources/AppDelegate.swift
git commit -m "chore(macos): clean up stale settings-removed comments"
```

---

### Task 9: Integration test — full end-to-end verification

**Step 1: Start Aleph Server with Control Plane**

Run: `cargo run --bin aleph-server --features control-plane`
Expected: Server starts, Control Plane available at http://127.0.0.1:18790

**Step 2: Verify Control Plane is accessible**

Run: `curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:18790/settings`
Expected: `200`

**Step 3: Build and run macOS App**

Run: `cd apps/macos && xcodebuild build -scheme Aleph -configuration Debug && open build/Debug/Aleph.app`
Or: Open Xcode, build and run.

**Step 4: Verify**

- [ ] Menu bar icon appears
- [ ] Cmd+, opens Settings window
- [ ] Settings window shows Leptos Control Plane UI
- [ ] Can navigate between settings pages (General, Providers, etc.)
- [ ] Changes made in WebView persist (saved to config.toml)
- [ ] Window is closable and reopenable
- [ ] If server stopped, error alert shows

**Step 5: Final commit**

```bash
git add -A && git commit -m "feat: Phase 1 complete — Settings UI unified via WebView + Control Plane"
```
