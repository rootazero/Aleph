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
}
