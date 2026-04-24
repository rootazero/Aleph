import AppKit
import ApplicationServices
import Foundation

// MARK: - Wire-format types (match Rust schema in aleph-protocol/methods/ax.rs)

/// Screen-coordinate bounding rectangle, top-left origin, in points.
/// Mirrors `aleph_protocol::desktop_bridge::methods::screen::Region`.
struct Region: Codable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

/// A node in the Accessibility element tree.
/// Mirrors `aleph_protocol::desktop_bridge::methods::ax::AxElement`.
struct AxElement: Codable {
    let role: String
    let title: String?
    let value: String?
    let bounds: Region?
    let pid: pid_t      // pid_t == Int32, serialises as JSON number
    var children: [AxElement]
}

// Params decoded from the JSON-RPC request
struct QueryTreeParams: Codable {
    var pid: Int32?
    var max_depth: Int?     // snake_case matches Rust JSON field name
}

struct QueryByRoleParams: Codable {
    var role: String
    var pid: Int32?
}

// Response envelopes
struct QueryResult: Codable {
    let element: AxElement?
}

struct QueryListResult: Codable {
    let elements: [AxElement]
}

// MARK: - AxQuerier actor

/// Serializes AX API calls and builds element trees.
///
/// All methods run on the actor's executor so that AX API calls
/// (which must not race) are naturally serialised.
actor AxQuerier {

    // MARK: Public interface

    func queryFocused() -> AxElement? {
        let sys = AXUIElementCreateSystemWide()
        var focused: AnyObject?
        let err = AXUIElementCopyAttributeValue(
            sys, kAXFocusedUIElementAttribute as CFString, &focused
        )
        guard err == .success, let el = focused else { return nil }
        // swiftlint:disable:next force_cast
        return buildElement(from: el as! AXUIElement, depth: 0, maxDepth: 2)
    }

    func queryTree(pid: pid_t?, maxDepth: Int) -> AxElement? {
        let target: AXUIElement
        if let p = pid {
            target = AXUIElementCreateApplication(p)
        } else {
            guard let app = NSWorkspace.shared.frontmostApplication else { return nil }
            target = AXUIElementCreateApplication(app.processIdentifier)
        }
        return buildElement(from: target, depth: 0, maxDepth: maxDepth)
    }

    func queryByRole(role: String, pid: pid_t?) -> [AxElement] {
        guard let root = queryTree(pid: pid, maxDepth: 8) else { return [] }
        return collectByRole(root, role: role)
    }

    // MARK: Private helpers

    private func collectByRole(_ el: AxElement, role: String) -> [AxElement] {
        var out: [AxElement] = []
        if el.role == role { out.append(el) }
        for child in el.children {
            out.append(contentsOf: collectByRole(child, role: role))
        }
        return out
    }

    private func buildElement(from ax: AXUIElement, depth: Int, maxDepth: Int) -> AxElement? {
        let role = (axAttr(ax, kAXRoleAttribute) as? String) ?? "AXUnknown"
        let title = axAttr(ax, kAXTitleAttribute) as? String
        let rawValue = axAttr(ax, kAXValueAttribute)
        // Represent value as string; nil for non-stringifiable types
        let value: String?
        if let rv = rawValue {
            let s = "\(rv)"
            value = s.isEmpty ? nil : s
        } else {
            value = nil
        }
        let bounds = boundsOf(ax)
        var ownerPid: pid_t = 0
        AXUIElementGetPid(ax, &ownerPid)

        var children: [AxElement] = []
        if depth < maxDepth {
            let rawChildren = axAttr(ax, kAXChildrenAttribute) as? [AXUIElement] ?? []
            children = rawChildren.compactMap {
                buildElement(from: $0, depth: depth + 1, maxDepth: maxDepth)
            }
        }
        return AxElement(
            role: role,
            title: title,
            value: value,
            bounds: bounds,
            pid: ownerPid,
            children: children
        )
    }

    private func axAttr(_ ax: AXUIElement, _ name: String) -> AnyObject? {
        var v: AnyObject?
        let err = AXUIElementCopyAttributeValue(ax, name as CFString, &v)
        return err == .success ? v : nil
    }

    private func boundsOf(_ ax: AXUIElement) -> Region? {
        var posVal: AnyObject?
        var sizeVal: AnyObject?
        guard AXUIElementCopyAttributeValue(ax, kAXPositionAttribute as CFString, &posVal) == .success,
              AXUIElementCopyAttributeValue(ax, kAXSizeAttribute as CFString, &sizeVal) == .success,
              let pv = posVal, let sv = sizeVal
        else { return nil }
        var point = CGPoint.zero
        var size  = CGSize.zero
        // swiftlint:disable:next force_cast
        AXValueGetValue(pv as! AXValue, .cgPoint, &point)
        // swiftlint:disable:next force_cast
        AXValueGetValue(sv as! AXValue, .cgSize, &size)
        return Region(
            x: Double(point.x),
            y: Double(point.y),
            width: Double(size.width),
            height: Double(size.height)
        )
    }
}
