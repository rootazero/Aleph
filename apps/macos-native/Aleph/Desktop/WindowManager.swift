import CoreGraphics
import AppKit
import Foundation

/// Window management using CoreGraphics and AppKit native APIs.
///
/// Provides window listing via `CGWindowListCopyWindowInfo`, window focusing
/// via `NSRunningApplication`, and app launching via the `open` command.
/// All results are returned as `AnyCodable` for the Desktop Bridge.
enum WindowManager {

    /// List all on-screen normal windows (layer 0).
    ///
    /// Uses `CGWindowListCopyWindowInfo` to enumerate windows that are currently
    /// visible on screen. Filters to layer 0 (normal application windows) and
    /// excludes windows without a name or owner.
    ///
    /// - Returns: An array of window info dictionaries, each containing:
    ///   `pid`, `app_name`, `window_id`, `title`, `bounds`.
    static func listWindows() -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let windowList = CGWindowListCopyWindowInfo(
            [.optionOnScreenOnly, .excludeDesktopElements],
            kCGNullWindowID
        ) as? [[String: Any]] else {
            return .failure(.init(
                code: .internal,
                message: "CGWindowListCopyWindowInfo returned nil"
            ))
        }

        var windows: [AnyCodable] = []

        for entry in windowList {
            // Filter to normal windows (layer 0)
            guard let layer = entry[kCGWindowLayer as String] as? Int,
                  layer == 0 else {
                continue
            }

            guard let pid = entry[kCGWindowOwnerPID as String] as? Int,
                  let appName = entry[kCGWindowOwnerName as String] as? String,
                  let windowId = entry[kCGWindowNumber as String] as? Int else {
                continue
            }

            let title = entry[kCGWindowName as String] as? String ?? ""

            // Extract bounds from the CGRect dictionary
            var boundsDict: [String: AnyCodable] = [:]
            if let bounds = entry[kCGWindowBounds as String] as? [String: Any] {
                boundsDict = [
                    "x": AnyCodable(bounds["X"] as? Double ?? 0.0),
                    "y": AnyCodable(bounds["Y"] as? Double ?? 0.0),
                    "width": AnyCodable(bounds["Width"] as? Double ?? 0.0),
                    "height": AnyCodable(bounds["Height"] as? Double ?? 0.0),
                ]
            }

            let windowInfo: [String: AnyCodable] = [
                "pid": AnyCodable(pid),
                "app_name": AnyCodable(appName),
                "window_id": AnyCodable(windowId),
                "title": AnyCodable(title),
                "bounds": AnyCodable(boundsDict),
            ]

            windows.append(AnyCodable(windowInfo))
        }

        return .success(AnyCodable(["windows": AnyCodable(windows)]))
    }

    /// Focus a window by bringing its owning application to the front.
    ///
    /// Uses `NSRunningApplication(processIdentifier:)` to locate the app
    /// and `activate(options:)` with `.activateIgnoringOtherApps` to bring
    /// it to the foreground.
    ///
    /// - Parameter pid: The process identifier of the target application.
    /// - Returns: A success acknowledgment or an error.
    static func focusWindow(pid: pid_t) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let app = NSRunningApplication(processIdentifier: pid) else {
            return .failure(.init(
                code: .internal,
                message: "No running application found with pid \(pid)"
            ))
        }

        let activated = app.activate(options: .activateIgnoringOtherApps)

        if activated {
            return .success(AnyCodable([
                "focused": AnyCodable(true),
                "pid": AnyCodable(Int(pid)),
                "app_name": AnyCodable(app.localizedName ?? "unknown"),
            ]))
        } else {
            return .failure(.init(
                code: .internal,
                message: "Failed to activate application with pid \(pid)"
            ))
        }
    }

    /// Launch an application by bundle identifier.
    ///
    /// Uses `Process` to run `open -b <bundleId>` which delegates to
    /// Launch Services for proper app activation.
    ///
    /// - Parameter bundleId: The bundle identifier (e.g. "com.apple.Safari").
    /// - Returns: A success acknowledgment or an error.
    static func launchApp(bundleId: String) -> Result<AnyCodable, BridgeServer.HandlerError> {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        process.arguments = ["-b", bundleId]

        let pipe = Pipe()
        process.standardError = pipe

        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return .failure(.init(
                code: .internal,
                message: "Failed to launch app: \(error.localizedDescription)"
            ))
        }

        if process.terminationStatus == 0 {
            return .success(AnyCodable([
                "launched": AnyCodable(true),
                "bundle_id": AnyCodable(bundleId),
            ]))
        } else {
            let errorData = pipe.fileHandleForReading.readDataToEndOfFile()
            let errorMessage = String(data: errorData, encoding: .utf8) ?? "unknown error"
            return .failure(.init(
                code: .internal,
                message: "open -b \(bundleId) failed (exit \(process.terminationStatus)): \(errorMessage)"
            ))
        }
    }
}
