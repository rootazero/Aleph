import Foundation
import CoreGraphics

/// Registers all desktop capability handlers on the BridgeServer.
///
/// This extension keeps handler registration centralized so that
/// `AppDelegate` (or tests) only needs to call `registerDesktopHandlers()`.
extension BridgeServer {

    /// Register all desktop capability handlers.
    func registerDesktopHandlers() {
        registerScreenshotHandler()
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
}
