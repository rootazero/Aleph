import AppKit
import CoreGraphics
import Foundation
#if canImport(ScreenCaptureKit)
import ScreenCaptureKit
#endif

/// Register `screen.capture` and `screen.list_displays` JSON-RPC handlers
/// that route screenshots through ScreenCaptureKit's `SCScreenshotManager`.
///
/// `SCScreenshotManager.captureImage(contentFilter:configuration:)` is
/// macOS 14+. Older systems return ERR_UNSUPPORTED so the Rust caller can
/// fall back to `xcap` via `NativeScreen`.
func registerScreenCaptureHandlers(_ router: Router) async {
    await router.register("screen.capture") { params in
        try await handleCapture(params: params)
    }

    await router.register("screen.list_displays") { _ in
        try await handleListDisplays()
    }
}

// MARK: - capture

private func handleCapture(params: JSONValue?) async throws -> JSONValue {
    guard #available(macOS 14.0, *) else {
        throw RpcError(
            code: -32601,
            message: "screen.capture: SCScreenshotManager requires macOS 14.0+",
            data: nil
        )
    }
    #if canImport(ScreenCaptureKit)
    let request = decodeCaptureParams(params)
    let display = try await selectDisplay(displayId: request.displayId)

    let filter = SCContentFilter(display: display, excludingWindows: [])

    let config = SCStreamConfiguration()
    if let region = request.region {
        // SCStreamConfiguration.sourceRect is in display points, top-left.
        config.sourceRect = CGRect(
            x: region.x, y: region.y, width: region.width, height: region.height
        )
        config.width = Int(region.width.rounded())
        config.height = Int(region.height.rounded())
    } else {
        config.width = display.width
        config.height = display.height
    }
    config.showsCursor = true

    let cgImage = try await SCScreenshotManager.captureImage(
        contentFilter: filter, configuration: config
    )

    let pngData = try pngData(from: cgImage)
    return .object([
        "png_base64": .string(pngData.base64EncodedString()),
        "width": .number(Double(cgImage.width)),
        "height": .number(Double(cgImage.height)),
    ])
    #else
    throw RpcError(
        code: -32601,
        message: "screen.capture: ScreenCaptureKit not available in this build",
        data: nil
    )
    #endif
}

// MARK: - list_displays

private func handleListDisplays() async throws -> JSONValue {
    guard #available(macOS 14.0, *) else {
        throw RpcError(
            code: -32601,
            message: "screen.list_displays: SCShareableContent requires macOS 14.0+",
            data: nil
        )
    }
    #if canImport(ScreenCaptureKit)
    let content = try await SCShareableContent.current
    let primaryID = CGMainDisplayID()
    let displays = content.displays.map { display -> JSONValue in
        encodeDisplayInfo(display, isPrimary: display.displayID == primaryID)
    }
    return .object(["displays": .array(displays)])
    #else
    throw RpcError(
        code: -32601,
        message: "screen.list_displays: ScreenCaptureKit not available in this build",
        data: nil
    )
    #endif
}

#if canImport(ScreenCaptureKit)
@available(macOS 14.0, *)
private func encodeDisplayInfo(_ display: SCDisplay, isPrimary: Bool) -> JSONValue {
    let frame = display.frame
    let bounds: JSONValue = .object([
        "x": .number(Double(frame.origin.x)),
        "y": .number(Double(frame.origin.y)),
        "width": .number(Double(frame.width)),
        "height": .number(Double(frame.height)),
    ])
    return .object([
        "id": .number(Double(display.displayID)),
        "bounds": bounds,
        "scale": .number(Double(scaleFactor(for: display.displayID))),
        "primary": .bool(isPrimary),
    ])
}
#endif

// MARK: - helpers

private struct CaptureRequest {
    let displayId: UInt32?
    let region: CaptureRegion?
}

private struct CaptureRegion {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

private func decodeCaptureParams(_ params: JSONValue?) -> CaptureRequest {
    guard case .object(let o) = params ?? .null else {
        return CaptureRequest(displayId: nil, region: nil)
    }
    var displayId: UInt32? = nil
    if case .number(let n) = o["display_id"] ?? .null {
        displayId = UInt32(n)
    }
    var region: CaptureRegion? = nil
    if case .object(let r) = o["region"] ?? .null,
       case .number(let x) = r["x"] ?? .null,
       case .number(let y) = r["y"] ?? .null,
       case .number(let w) = r["width"] ?? .null,
       case .number(let h) = r["height"] ?? .null,
       w > 0, h > 0 {
        region = CaptureRegion(x: x, y: y, width: w, height: h)
    }
    return CaptureRequest(displayId: displayId, region: region)
}

#if canImport(ScreenCaptureKit)
@available(macOS 14.0, *)
private func selectDisplay(displayId: UInt32?) async throws -> SCDisplay {
    let content = try await SCShareableContent.current
    guard !content.displays.isEmpty else {
        throw RpcError(
            code: -32004,
            message: "screen.capture: no displays available",
            data: nil
        )
    }
    if let id = displayId,
       let match = content.displays.first(where: { $0.displayID == id }) {
        return match
    }
    let primaryID = CGMainDisplayID()
    return content.displays.first(where: { $0.displayID == primaryID })
        ?? content.displays[0]
}
#endif

private func scaleFactor(for displayID: CGDirectDisplayID) -> CGFloat {
    let screen = NSScreen.screens.first(where: {
        ($0.deviceDescription[
            NSDeviceDescriptionKey("NSScreenNumber")
        ] as? NSNumber)?.uint32Value == displayID
    })
    return screen?.backingScaleFactor ?? 1.0
}

private func pngData(from image: CGImage) throws -> Data {
    let rep = NSBitmapImageRep(cgImage: image)
    guard let data = rep.representation(using: .png, properties: [:]) else {
        throw RpcError(
            code: -32003,
            message: "screen.capture: PNG encoding failed",
            data: nil
        )
    }
    return data
}
