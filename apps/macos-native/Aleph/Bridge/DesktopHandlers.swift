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
}
