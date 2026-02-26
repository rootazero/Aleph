import ApplicationServices
import Cocoa

/// Accessibility tree inspection using native AXUIElement APIs.
///
/// Provides permission checking and recursive AX tree traversal for
/// any running application (by bundle ID or frontmost). Returns a
/// nested dictionary suitable for the Desktop Bridge `desktop.ax_tree`.
///
/// This is the Swift equivalent of the Tauri `bridge/perception.rs::macos_ax_tree()`,
/// replacing ~180 lines of C FFI with idiomatic Swift.
enum AccessibilityService {

    /// Default maximum depth for AX tree traversal.
    private static let defaultMaxDepth = 5

    /// Maximum number of children to visit per node (prevents explosion).
    private static let maxChildrenPerNode = 128

    // MARK: - Permission

    /// Check whether this process has accessibility permission.
    static var hasPermission: Bool {
        AXIsProcessTrusted()
    }

    /// Prompt the user to grant accessibility permission (shows system dialog).
    static func requestPermission() {
        let opts = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): true] as CFDictionary
        AXIsProcessTrustedWithOptions(opts)
    }

    // MARK: - Public API

    /// Inspect the AX tree for a running application.
    ///
    /// - Parameters:
    ///   - bundleId: Bundle identifier (e.g. "com.apple.Safari"). If `nil`, uses the frontmost app.
    ///   - maxDepth: Maximum recursion depth (default 5).
    /// - Returns: A nested dictionary with `role`, `title`, `value`, and `children` keys,
    ///   wrapped in `AnyCodable`, or a `HandlerError` on failure.
    static func inspect(
        bundleId: String? = nil,
        maxDepth: Int = 5
    ) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard hasPermission else {
            return .failure(.init(
                code: .internal,
                message: "Accessibility permission not granted"
            ))
        }

        let pid: pid_t
        if let bundleId = bundleId {
            guard let app = NSRunningApplication.runningApplications(
                withBundleIdentifier: bundleId
            ).first else {
                return .failure(.init(
                    code: .internal,
                    message: "No running app found with bundle ID: \(bundleId)"
                ))
            }
            pid = app.processIdentifier
        } else {
            guard let app = NSWorkspace.shared.frontmostApplication else {
                return .failure(.init(
                    code: .internal,
                    message: "No frontmost application found"
                ))
            }
            pid = app.processIdentifier
        }

        let appElement = AXUIElementCreateApplication(pid)
        let tree = walkTree(element: appElement, depth: 0, maxDepth: max(maxDepth, 1))
        return .success(tree)
    }

    // MARK: - Tree Traversal

    /// Recursively walk the AX element tree, building a nested AnyCodable dictionary.
    private static func walkTree(
        element: AXUIElement,
        depth: Int,
        maxDepth: Int
    ) -> AnyCodable {
        if depth >= maxDepth {
            return AnyCodable(["truncated": AnyCodable(true)])
        }

        var result: [String: AnyCodable] = [:]

        // Core attributes
        if let role = getAttribute(element, kAXRoleAttribute as String) as? String {
            result["role"] = AnyCodable(role)
        }
        if let title = getAttribute(element, kAXTitleAttribute as String) as? String, !title.isEmpty {
            result["title"] = AnyCodable(title)
        }
        if let value = getAttribute(element, kAXValueAttribute as String) as? String, !value.isEmpty {
            result["value"] = AnyCodable(value)
        }
        if let desc = getAttribute(element, kAXDescriptionAttribute as String) as? String, !desc.isEmpty {
            result["description"] = AnyCodable(desc)
        }

        // Recurse into children
        if depth < maxDepth,
           let children = getAttribute(element, kAXChildrenAttribute as String) as? [AXUIElement] {
            let childTrees: [AnyCodable] = children.prefix(maxChildrenPerNode).map {
                walkTree(element: $0, depth: depth + 1, maxDepth: maxDepth)
            }
            if !childTrees.isEmpty {
                result["children"] = AnyCodable(childTrees)
            }
        }

        return AnyCodable(result)
    }

    // MARK: - Attribute Helpers

    /// Read a single attribute from an AXUIElement.
    ///
    /// Returns `nil` when the attribute is unsupported or the call fails.
    private static func getAttribute(_ element: AXUIElement, _ attribute: String) -> CFTypeRef? {
        var value: CFTypeRef?
        let err = AXUIElementCopyAttributeValue(element, attribute as CFString, &value)
        return err == .success ? value : nil
    }
}
