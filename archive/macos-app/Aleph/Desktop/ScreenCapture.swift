import CoreGraphics
import AppKit
import Foundation

/// Screen capture using CoreGraphics native APIs.
///
/// Provides permission checking and full-screen or region capture,
/// returning base64-encoded PNG data suitable for the Desktop Bridge.
enum ScreenCapture {

    /// Check screen recording permission without triggering a prompt.
    static var hasPermission: Bool {
        CGPreflightScreenCaptureAccess()
    }

    /// Request screen recording permission (shows system dialog on first call).
    @discardableResult
    static func requestPermission() -> Bool {
        CGRequestScreenCaptureAccess()
    }

    /// Capture the full screen or a specified region.
    ///
    /// - Parameter region: Optional rectangle to capture. Pass `nil` for full screen.
    /// - Returns: A dictionary containing `image` (base64 PNG), `width`, and `height`,
    ///   wrapped in `AnyCodable`, or a `HandlerError` on failure.
    static func capture(region: CGRect? = nil) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard hasPermission else {
            return .failure(.init(
                code: .internal,
                message: "Screen Recording permission not granted"
            ))
        }

        let rect = region ?? CGRect.infinite
        guard let image = CGWindowListCreateImage(
            rect,
            .optionOnScreenOnly,
            kCGNullWindowID,
            .bestResolution
        ) else {
            return .failure(.init(
                code: .internal,
                message: "Failed to capture screen"
            ))
        }

        let bitmapRep = NSBitmapImageRep(cgImage: image)
        guard let pngData = bitmapRep.representation(using: .png, properties: [:]) else {
            return .failure(.init(
                code: .internal,
                message: "Failed to encode PNG"
            ))
        }

        let result: [String: AnyCodable] = [
            "image": AnyCodable(pngData.base64EncodedString()),
            "width": AnyCodable(image.width),
            "height": AnyCodable(image.height),
        ]
        return .success(AnyCodable(result))
    }
}
