import Foundation
import CoreGraphics
import AppKit

/// Registers all desktop capability handlers on the BridgeServer.
///
/// This extension keeps handler registration centralized so that
/// `AppDelegate` (or tests) only needs to call `registerDesktopHandlers()`.
extension BridgeServer {

    /// Register all desktop capability handlers.
    func registerDesktopHandlers() {
        registerScreenshotHandler()
        registerOCRHandler()
        registerAxTreeHandler()
        registerWindowListHandler()
        registerFocusWindowHandler()
        registerLaunchAppHandler()
        registerClickHandler()
        registerTypeTextHandler()
        registerKeyComboHandler()
        registerScrollHandler()
        registerTrayUpdateStatusHandler()
        registerWebviewHandlers()
        registerCanvasHandlers()
    }

    // MARK: - Screenshot

    private func registerScreenshotHandler() {
        register(method: BridgeMethod.screenshot.rawValue) { params in
            // Parse optional region from params
            let region: CGRect?
            if let x = params["x"]?.doubleValue,
               let y = params["y"]?.doubleValue,
               let width = params["width"]?.doubleValue,
               let height = params["height"]?.doubleValue {
                region = CGRect(x: x, y: y, width: width, height: height)
            } else {
                region = nil
            }

            return ScreenCapture.capture(region: region)
        }
    }

    // MARK: - OCR

    /// Register `desktop.ocr` handler.
    ///
    /// Params:
    /// - `image_base64` (optional): Base64-encoded image. If absent, captures the screen first.
    ///
    /// Returns: `{ "text": "...", "lines": [{ "text": "...", "confidence": 0.95 }] }`
    private func registerOCRHandler() {
        register(method: BridgeMethod.ocr.rawValue) { params in
            if let imageBase64 = params["image_base64"]?.stringValue {
                // OCR from provided image
                return OCRService.recognize(imageBase64: imageBase64).map { $0.asAnyCodable }
            } else {
                // No image provided — capture screen first, then OCR
                guard ScreenCapture.hasPermission else {
                    return .failure(.init(
                        code: .internal,
                        message: "Screen Recording permission not granted. "
                            + "Enable in System Settings > Privacy & Security > Screen Recording."
                    ))
                }

                guard let cgImage = CGWindowListCreateImage(
                    CGRect.null,
                    .optionOnScreenOnly,
                    kCGNullWindowID,
                    [.bestResolution]
                ) else {
                    return .failure(.init(
                        code: .internal,
                        message: "Screen capture failed"
                    ))
                }

                let bitmapRep = NSBitmapImageRep(cgImage: cgImage)
                guard let pngData = bitmapRep.representation(using: .png, properties: [:]) else {
                    return .failure(.init(
                        code: .internal,
                        message: "Failed to encode screenshot as PNG"
                    ))
                }

                return OCRService.recognize(imageData: pngData).map { $0.asAnyCodable }
            }
        }
    }

    // MARK: - AX Tree

    private func registerAxTreeHandler() {
        register(method: BridgeMethod.axTree.rawValue) { params in
            // Match Rust param name: "app_bundle_id"
            let bundleId = params["app_bundle_id"]?.stringValue
            let maxDepth = params["max_depth"]?.intValue ?? 5

            return AccessibilityService.inspect(bundleId: bundleId, maxDepth: maxDepth)
        }
    }

    // MARK: - Window List

    /// Register `desktop.window_list` handler.
    ///
    /// No params required. Returns an array of on-screen normal windows
    /// with pid, app_name, window_id, title, and bounds.
    private func registerWindowListHandler() {
        register(method: BridgeMethod.windowList.rawValue) { _ in
            WindowManager.listWindows()
        }
    }

    // MARK: - Focus Window

    /// Register `desktop.focus_window` handler.
    ///
    /// Params:
    /// - `pid` (required): Process identifier of the target application.
    private func registerFocusWindowHandler() {
        register(method: BridgeMethod.focusWindow.rawValue) { params in
            guard let pid = params["pid"]?.intValue else {
                return .failure(.init(
                    code: .internal,
                    message: "Missing required param: pid (integer)"
                ))
            }

            return WindowManager.focusWindow(pid: pid_t(pid))
        }
    }

    // MARK: - Launch App

    /// Register `desktop.launch_app` handler.
    ///
    /// Params:
    /// - `bundle_id` (required): Bundle identifier of the app to launch.
    private func registerLaunchAppHandler() {
        register(method: BridgeMethod.launchApp.rawValue) { params in
            guard let bundleId = params["bundle_id"]?.stringValue else {
                return .failure(.init(
                    code: .internal,
                    message: "Missing required param: bundle_id (string)"
                ))
            }

            return WindowManager.launchApp(bundleId: bundleId)
        }
    }

    // MARK: - Click

    /// Register `desktop.click` handler.
    ///
    /// Params:
    /// - `x` (required): Screen X coordinate (double).
    /// - `y` (required): Screen Y coordinate (double).
    /// - `button` (optional): `"left"` (default), `"right"`, or `"middle"`.
    private func registerClickHandler() {
        register(method: BridgeMethod.click.rawValue) { params in
            guard let x = params["x"]?.doubleValue else {
                return .failure(.init(
                    code: .internal,
                    message: "Missing or invalid 'x' parameter"
                ))
            }
            guard let y = params["y"]?.doubleValue else {
                return .failure(.init(
                    code: .internal,
                    message: "Missing or invalid 'y' parameter"
                ))
            }
            let button = params["button"]?.stringValue ?? "left"

            return InputAutomation.click(x: x, y: y, button: button)
        }
    }

    // MARK: - Type Text

    /// Register `desktop.type_text` handler.
    ///
    /// Params:
    /// - `text` (required): The text string to type.
    private func registerTypeTextHandler() {
        register(method: BridgeMethod.typeText.rawValue) { params in
            guard let text = params["text"]?.stringValue else {
                return .failure(.init(
                    code: .internal,
                    message: "Missing or invalid 'text' parameter"
                ))
            }

            return InputAutomation.typeText(text)
        }
    }

    // MARK: - Key Combo

    /// Register `desktop.key_combo` handler.
    ///
    /// Accepts two formats:
    /// 1. New format: `{ "modifiers": ["meta", "shift"], "key": "c" }`
    /// 2. Legacy format: `{ "keys": ["cmd", "c"] }` — last element is the main key.
    ///
    /// Modifier names: "meta"/"command"/"cmd"/"super", "shift", "control"/"ctrl", "alt"/"option".
    private func registerKeyComboHandler() {
        register(method: BridgeMethod.keyCombo.rawValue) { params in
            // Try legacy format first: flat "keys" array
            if let keysArray = params["keys"]?.arrayValue {
                let strs = keysArray.compactMap { $0.stringValue }
                guard !strs.isEmpty else {
                    return .failure(.init(
                        code: .internal,
                        message: "Empty 'keys' array"
                    ))
                }

                // Last element is the main key, all preceding are modifiers
                let modifiers = Array(strs.dropLast())
                let key = strs.last!

                return InputAutomation.keyCombo(modifiers: modifiers, key: key)
            }

            // New format: separate "modifiers" + "key"
            guard let key = params["key"]?.stringValue else {
                return .failure(.init(
                    code: .internal,
                    message: "Missing 'key' parameter (or use legacy 'keys' array)"
                ))
            }

            let modifiers: [String]
            if let modArray = params["modifiers"]?.arrayValue {
                modifiers = modArray.compactMap { $0.stringValue }
            } else {
                modifiers = []
            }

            return InputAutomation.keyCombo(modifiers: modifiers, key: key)
        }
    }

    // MARK: - Scroll

    /// Register `desktop.scroll` handler.
    ///
    /// Params:
    /// - `direction` (optional): `"up"`, `"down"` (default), `"left"`, or `"right"`.
    /// - `amount` (optional): Number of scroll ticks (default 3).
    private func registerScrollHandler() {
        register(method: BridgeMethod.scroll.rawValue) { params in
            let direction = params["direction"]?.stringValue ?? "down"
            let amount = Int32(params["amount"]?.intValue ?? 3)

            return InputAutomation.scroll(direction: direction, amount: amount)
        }
    }

    // MARK: - Tray Update Status

    /// Register `tray.update_status` handler.
    ///
    /// Params:
    /// - `status` (optional): Agent status string ("idle", "thinking", "acting", "error"). Default "idle".
    /// - `tooltip` (optional): Explicit tooltip override.
    ///
    /// Posts `Notification.Name.updateTrayStatus` on the main thread so
    /// `MenuBarController` (or `AppDelegate`) can update the status item.
    private func registerTrayUpdateStatusHandler() {
        register(method: BridgeMethod.trayUpdateStatus.rawValue) { params in
            let status = params["status"]?.stringValue ?? "idle"
            let tooltip = params["tooltip"]?.stringValue

            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .updateTrayStatus,
                    object: nil,
                    userInfo: ["status": status, "tooltip": tooltip as Any]
                )
            }

            return .success(AnyCodable(["updated": AnyCodable(true), "status": AnyCodable(status)]))
        }
    }

    // MARK: - WebView Handlers

    /// Register `webview.show`, `webview.hide`, and `webview.navigate` handlers.
    ///
    /// These handlers post notifications so that the UI layer (HaloWindow,
    /// SettingsWindow) can respond on the main thread.
    private func registerWebviewHandlers() {
        // webview.show — show a named webview (halo or settings)
        // Params: { "label": "halo"|"settings", "url": "http://..." (optional) }
        register(method: BridgeMethod.webviewShow.rawValue) { params in
            let label = params["label"]?.stringValue ?? "halo"
            let url = params["url"]?.stringValue

            var userInfo: [String: Any] = ["label": label]
            if let url = url { userInfo["url"] = url }

            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .webviewShow,
                    object: nil,
                    userInfo: userInfo
                )
            }

            return .success(AnyCodable(["shown": AnyCodable(true), "label": AnyCodable(label)]))
        }

        // webview.hide — hide a named webview
        // Params: { "label": "halo"|"settings" }
        register(method: BridgeMethod.webviewHide.rawValue) { params in
            let label = params["label"]?.stringValue ?? "halo"

            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .webviewHide,
                    object: nil,
                    userInfo: ["label": label]
                )
            }

            return .success(AnyCodable(["hidden": AnyCodable(true), "label": AnyCodable(label)]))
        }

        // webview.navigate — navigate a named webview to a URL
        // Params: { "label": "halo"|"settings", "url": "http://..." }
        register(method: BridgeMethod.webviewNavigate.rawValue) { params in
            let label = params["label"]?.stringValue ?? "halo"
            guard let url = params["url"]?.stringValue else {
                return .failure(.init(
                    code: .internal,
                    message: "Missing required param: url (string)"
                ))
            }

            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .webviewNavigate,
                    object: nil,
                    userInfo: ["label": label, "url": url]
                )
            }

            return .success(AnyCodable(["navigated": AnyCodable(true), "label": AnyCodable(label)]))
        }
    }

    // MARK: - Canvas Handlers

    /// Register `desktop.canvas_show`, `desktop.canvas_hide`, and
    /// `desktop.canvas_update` handlers.
    ///
    /// These handlers post notifications so that CanvasOverlay can respond
    /// on the main thread.
    private func registerCanvasHandlers() {
        // desktop.canvas_show — show the canvas overlay with HTML and position
        // Params: { "html": "<h1>Hi</h1>", "position": { "x": 100, "y": 100, "width": 400, "height": 300 } }
        register(method: BridgeMethod.canvasShow.rawValue) { params in
            let html = params["html"]?.stringValue ?? "<html><body></body></html>"

            let pos = params["position"]?.dictValue
            let x = pos?["x"]?.doubleValue ?? 100.0
            let y = pos?["y"]?.doubleValue ?? 100.0
            let width = pos?["width"]?.doubleValue ?? 400.0
            let height = pos?["height"]?.doubleValue ?? 600.0

            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .canvasShow,
                    object: nil,
                    userInfo: [
                        "html": html,
                        "x": x,
                        "y": y,
                        "width": width,
                        "height": height,
                    ]
                )
            }

            return .success(AnyCodable([
                "visible": AnyCodable(true),
                "position": AnyCodable([
                    "x": AnyCodable(x),
                    "y": AnyCodable(y),
                    "width": AnyCodable(width),
                    "height": AnyCodable(height),
                ]),
            ]))
        }

        // desktop.canvas_hide — hide the canvas overlay
        register(method: BridgeMethod.canvasHide.rawValue) { _ in
            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .canvasHide,
                    object: nil
                )
            }

            return .success(AnyCodable(["visible": AnyCodable(false)]))
        }

        // desktop.canvas_update — apply an A2UI patch to the canvas
        // Params: { "patch": [{"type": "surfaceUpdate", "content": "<p>Updated</p>"}] }
        register(method: BridgeMethod.canvasUpdate.rawValue) { params in
            guard let patchArray = params["patch"]?.arrayValue else {
                return .failure(.init(
                    code: .internal,
                    message: "Missing required param: patch (array)"
                ))
            }

            // Serialize patch back to JSON string for JS evaluation
            let encoder = JSONEncoder()
            guard let patchData = try? encoder.encode(patchArray),
                  let patchJSON = String(data: patchData, encoding: .utf8) else {
                return .failure(.init(
                    code: .internal,
                    message: "Failed to serialize patch to JSON"
                ))
            }

            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .canvasUpdate,
                    object: nil,
                    userInfo: ["patch": patchJSON]
                )
            }

            return .success(AnyCodable(["patched": AnyCodable(true)]))
        }
    }
}

// MARK: - Notification Names (WebView + Canvas)

extension Notification.Name {
    /// Posted by `webview.show` handler. UserInfo: `["label": String, "url": String?]`.
    static let webviewShow = Notification.Name("com.aleph.webviewShow")

    /// Posted by `webview.hide` handler. UserInfo: `["label": String]`.
    static let webviewHide = Notification.Name("com.aleph.webviewHide")

    /// Posted by `webview.navigate` handler. UserInfo: `["label": String, "url": String]`.
    static let webviewNavigate = Notification.Name("com.aleph.webviewNavigate")

    /// Posted by `desktop.canvas_show` handler. UserInfo: `["html": String, "x": Double, "y": Double, "width": Double, "height": Double]`.
    static let canvasShow = Notification.Name("com.aleph.canvasShow")

    /// Posted by `desktop.canvas_hide` handler.
    static let canvasHide = Notification.Name("com.aleph.canvasHide")

    /// Posted by `desktop.canvas_update` handler. UserInfo: `["patch": String]` (JSON string).
    static let canvasUpdate = Notification.Name("com.aleph.canvasUpdate")
}
