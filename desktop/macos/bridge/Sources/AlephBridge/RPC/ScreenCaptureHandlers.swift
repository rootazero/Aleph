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
///
/// `screen.capture` has two rails:
/// - **display** (`display_id` / `region`, or neither): the whole display, as
///   before.
/// - **window** (`window_id`): only that window, captured off the desktop
///   compositor so it comes back even when occluded or partially offscreen —
///   and so no *other* window of the user's leaks into the model's context.
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
    // The model is meant to see the app, not the user's pointer: the cursor is
    // composited in only when the caller explicitly asks for it.
    let showsCursor = request.showCursor ?? false

    if let windowId = request.windowId {
        // `region` is a rectangle in DISPLAY coordinates; it has no meaning
        // against a window-scoped filter. Reject the combination instead of
        // silently dropping one of the two.
        guard request.region == nil else {
            throw RpcError(
                code: -32602,
                message: "screen.capture: `window_id` and `region` are mutually exclusive",
                data: nil
            )
        }
        return try await captureWindow(windowId: windowId, showsCursor: showsCursor)
    }

    return try await captureDisplay(
        displayId: request.displayId,
        region: request.region,
        showsCursor: showsCursor
    )
    #else
    throw RpcError(
        code: -32601,
        message: "screen.capture: ScreenCaptureKit not available in this build",
        data: nil
    )
    #endif
}

#if canImport(ScreenCaptureKit)
/// Capture one window, identified by the `id` `window.list` reported.
///
/// `SCContentFilter(desktopIndependentWindow:)` renders the window off the
/// desktop compositor, so the capture succeeds even when the window is behind
/// another one or hangs off the edge of the display — which is exactly the
/// state a background-driven app is in. A display filter + crop could do
/// neither: it would composite the occluding window into the frame.
@available(macOS 14.0, *)
private func captureWindow(windowId: CGWindowID, showsCursor: Bool) async throws -> JSONValue {
    let content = try await SCShareableContent.current
    guard let window = content.windows.first(where: { $0.windowID == windowId }) else {
        // Degrading to a full-display capture here would quietly reintroduce
        // both defects this rail exists to fix (the occluder comes back, and
        // every other window on the display leaks into the model's context),
        // and the model would have no way to tell that happened. Fail instead:
        // a stale window id is recoverable by re-running `window.list`.
        throw RpcError(
            code: -32004,
            message: "screen.capture: no window with id \(windowId) (it may have closed; re-run window.list)",
            data: nil
        )
    }

    let frame = window.frame // global screen POINTS, top-left origin
    guard frame.width > 0, frame.height > 0 else {
        throw RpcError(
            code: -32004,
            message: "screen.capture: window \(windowId) has an empty frame",
            data: nil
        )
    }

    let scale = backingScale(forWindowFrame: frame, in: content)

    let filter = SCContentFilter(desktopIndependentWindow: window)

    let config = SCStreamConfiguration()
    // Output surface is sized in PIXELS; the frame is in POINTS.
    config.width = Int((frame.width * scale).rounded())
    config.height = Int((frame.height * scale).rounded())
    config.showsCursor = showsCursor
    // The drop shadow is painted OUTSIDE the window frame. Keeping it would
    // pad the image against the frame we report as `window_bounds`, and every
    // pixel→point mapping below would be off by the shadow inset.
    config.ignoreShadowsSingleWindow = true

    let cgImage = try await SCScreenshotManager.captureImage(
        contentFilter: filter, configuration: config
    )
    let png = try pngData(from: cgImage)

    return .object([
        "png_base64": .string(png.base64EncodedString()),
        "width": .number(Double(cgImage.width)),
        "height": .number(Double(cgImage.height)),
        // LOAD-BEARING. The model sees window-relative pixels, but every click
        // is issued in the global point space, so the mapping has to travel
        // with the image rather than be re-derived downstream:
        //   global_point = window_bounds.origin + pixel / scale
        "window_bounds": .object([
            "x": .number(Double(frame.origin.x)),
            "y": .number(Double(frame.origin.y)),
            "width": .number(Double(frame.width)),
            "height": .number(Double(frame.height)),
        ]),
        "scale": .number(observedScale(pixelWidth: cgImage.width, pointWidth: frame.width)),
    ])
}

/// Capture a whole display (optionally a sub-rectangle of it) — the original
/// behaviour, minus the hardcoded cursor.
@available(macOS 14.0, *)
private func captureDisplay(
    displayId: UInt32?,
    region: CaptureRegion?,
    showsCursor: Bool
) async throws -> JSONValue {
    let display = try await selectDisplay(displayId: displayId)

    let filter = SCContentFilter(display: display, excludingWindows: [])

    // Point width of the surface we are asking for; the denominator of the
    // scale we report below.
    let pointWidth: CGFloat = region.map { CGFloat($0.width) } ?? display.frame.width

    let config = SCStreamConfiguration()
    if let region {
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
    config.showsCursor = showsCursor

    let cgImage = try await SCScreenshotManager.captureImage(
        contentFilter: filter, configuration: config
    )

    let pngData = try pngData(from: cgImage)
    return .object([
        "png_base64": .string(pngData.base64EncodedString()),
        "width": .number(Double(cgImage.width)),
        "height": .number(Double(cgImage.height)),
        // Reported so `pixel / scale` is a valid mapping on this rail too,
        // instead of a window-capture special case that callers have to know
        // about. Measured, not asserted — see `observedScale`.
        "scale": .number(observedScale(pixelWidth: cgImage.width, pointWidth: pointWidth)),
    ])
}
#endif

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
    let windowId: CGWindowID?
    /// `nil` ⇒ caller did not say; the handler decides (and decides "no").
    let showCursor: Bool?
}

private struct CaptureRegion {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

private func decodeCaptureParams(_ params: JSONValue?) -> CaptureRequest {
    guard case .object(let o) = params ?? .null else {
        return CaptureRequest(displayId: nil, region: nil, windowId: nil, showCursor: nil)
    }
    var displayId: UInt32? = nil
    if case .number(let n) = o["display_id"] ?? .null {
        displayId = UInt32(n)
    }
    var windowId: CGWindowID? = nil
    // A window id arrives as a JSON number; only a value that actually fits a
    // CGWindowID is one — anything else would trap on conversion.
    if case .number(let n) = o["window_id"] ?? .null,
       n >= 0, n <= Double(CGWindowID.max), n == n.rounded() {
        windowId = CGWindowID(n)
    }
    var showCursor: Bool? = nil
    if case .bool(let b) = o["show_cursor"] ?? .null {
        showCursor = b
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
    return CaptureRequest(
        displayId: displayId, region: region, windowId: windowId, showCursor: showCursor
    )
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

/// Backing scale of the display the window sits on.
///
/// Purely geometric: the window's centre point selects the display whose frame
/// contains it (`SCWindow.frame` and `SCDisplay.frame` share the global,
/// top-left-origin point space). No guessing about which window matters — the
/// caller already named the window.
@available(macOS 14.0, *)
private func backingScale(forWindowFrame frame: CGRect, in content: SCShareableContent) -> CGFloat {
    let centre = CGPoint(x: frame.midX, y: frame.midY)
    if let display = content.displays.first(where: { $0.frame.contains(centre) }) {
        return scaleFactor(for: display.displayID)
    }
    // A window parked offscreen belongs to no display frame; the main display's
    // scale is the only defensible answer left.
    return scaleFactor(for: CGMainDisplayID())
}
#endif

/// The scale the capture ACTUALLY came back at: image pixels ÷ surface points,
/// measured off the returned image rather than off the value we asked for.
///
/// Asserting a scale here would be a guess about ScreenCaptureKit's units, and
/// the two rails ask for different things: the display rail sizes the output
/// surface from `SCDisplay.width` (points) while the window rail multiplies the
/// frame by the backing scale — and SCK is free to clamp or round either. The
/// ratio between the two numbers we hold at the end (`cgImage.width` in pixels,
/// the frame in points) is the one thing that cannot be wrong, and it is
/// precisely what the caller needs: `global_point = origin + pixel / scale`.
/// Getting this off by 2x on a Retina display puts every click somewhere else.
private func observedScale(pixelWidth: Int, pointWidth: CGFloat) -> Double {
    // A degenerate surface has no meaningful ratio; 1.0 maps pixels to points
    // one-to-one, which is the only non-destructive answer.
    guard pixelWidth > 0, pointWidth > 0 else { return 1.0 }
    return Double(pixelWidth) / Double(pointWidth)
}

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
