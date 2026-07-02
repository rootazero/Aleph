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

/// Stateless element locator for `ax.set_value` / `ax.perform_action`.
/// Mirrors `aleph_protocol::desktop_bridge::methods::ax::AxLocator`.
struct AxLocator: Codable {
    var pid: Int32?
    var role: String?
    var title: String?
    var center: [Double]?
}

struct SetValueParams: Codable {
    var locator: AxLocator
    var value: String
}

struct PerformActionParams: Codable {
    var locator: AxLocator
    var action: String
}

/// Post-write verification outcome.
/// Mirrors `aleph_protocol::desktop_bridge::methods::ax::AxVerification`.
struct AxVerification: Codable {
    let state: String
    let reason: String?
    let actual_preview: String?
}

/// Result for `ax.set_value` and `ax.perform_action`.
/// Mirrors `aleph_protocol::desktop_bridge::methods::ax::AxActionResult`.
struct AxActionResult: Codable {
    let performed: Bool
    let path: String
    let matched: AxElement?
    let verification: AxVerification?
}

/// Pure locator scoring — testable without live AX handles.
/// Higher is better; `nil` means "does not match at all".
///
/// Scoring: role filter is a hard reject; title match adds 100 (exact) or 50
/// (contains, case-insensitive); center proximity adds up to 100, decaying
/// with distance, as a tiebreak among otherwise-equal matches.
func locatorScore(
    locator: AxLocator,
    role: String,
    title: String?,
    bounds: Region?
) -> Double? {
    if let wantRole = locator.role, wantRole != role { return nil }
    var score = 0.0
    if let wantTitle = locator.title {
        guard let t = title?.lowercased() else { return nil }
        let w = wantTitle.lowercased()
        if t == w { score += 100 }
        else if t.contains(w) { score += 50 }
        else { return nil }
    }
    if let c = locator.center, c.count == 2, let b = bounds {
        let cx = b.x + b.width / 2, cy = b.y + b.height / 2
        let dist = ((cx - c[0]) * (cx - c[0]) + (cy - c[1]) * (cy - c[1])).squareRoot()
        score += max(0, 100 - dist / 10) // within ~1000pt still differentiates
    }
    return score
}

// MARK: - AxQuerier actor

/// Serializes AX API calls and builds element trees.
///
/// All methods run on the actor's executor so that AX API calls
/// (which must not race) are naturally serialised.
actor AxQuerier {

    private let MAX_TREE_NODES = 10_000

    // MARK: Public interface

    func queryFocused() -> AxElement? {
        let sys = AXUIElementCreateSystemWide()
        var focused: AnyObject?
        let err = AXUIElementCopyAttributeValue(
            sys, kAXFocusedUIElementAttribute as CFString, &focused
        )
        guard err == .success, let el = focused else { return nil }
        var count = 0
        // swiftlint:disable:next force_cast
        return buildElement(from: el as! AXUIElement, depth: 0, maxDepth: 2, nodeCount: &count)
    }

    func queryTree(pid: pid_t?, maxDepth: Int) -> AxElement? {
        let target: AXUIElement
        if let p = pid {
            target = AXUIElementCreateApplication(p)
        } else {
            guard let app = NSWorkspace.shared.frontmostApplication else { return nil }
            target = AXUIElementCreateApplication(app.processIdentifier)
        }
        var count = 0
        return buildElement(from: target, depth: 0, maxDepth: maxDepth, nodeCount: &count)
    }

    func queryByRole(role: String, pid: pid_t?) -> [AxElement] {
        guard let root = queryTree(pid: pid, maxDepth: 8) else { return [] }
        return collectByRole(root, role: role)
    }

    func setValue(_ params: SetValueParams) throws -> AxActionResult {
        guard let (handle, meta) = locate(params.locator) else {
            throw RpcError(code: -32_602, message: "no element matches locator", data: nil)
        }
        let err = AXUIElementSetAttributeValue(
            handle, kAXValueAttribute as CFString, params.value as CFTypeRef
        )
        guard err == .success else {
            throw RpcError(
                code: -32_603,
                message: "AXUIElementSetAttributeValue failed: \(err.rawValue) (element may be read-only)",
                data: nil
            )
        }
        // Read back for verification.
        var readBack: AnyObject?
        let readErr = AXUIElementCopyAttributeValue(handle, kAXValueAttribute as CFString, &readBack)
        let actual = readErr == .success ? readBack.map { "\($0)" } : nil
        let verification: AxVerification
        if let a = actual {
            verification = a == params.value
                ? AxVerification(state: "verified", reason: nil, actual_preview: nil)
                : AxVerification(
                    state: "unverified", reason: "value_mismatch",
                    actual_preview: String(a.prefix(200))
                )
        } else {
            verification = AxVerification(state: "unverified", reason: "value_unreadable", actual_preview: nil)
        }
        return AxActionResult(performed: true, path: "accessibility", matched: meta, verification: verification)
    }

    func performAction(_ params: PerformActionParams) throws -> AxActionResult {
        guard let (handle, meta) = locate(params.locator) else {
            throw RpcError(code: -32_602, message: "no element matches locator", data: nil)
        }
        let err = AXUIElementPerformAction(handle, params.action as CFString)
        guard err == .success else {
            throw RpcError(
                code: -32_603,
                message: "AXUIElementPerformAction(\(params.action)) failed: \(err.rawValue)",
                data: nil
            )
        }
        return AxActionResult(performed: true, path: "accessibility", matched: meta, verification: nil)
    }

    // MARK: Private helpers

    /// Walk the AX tree keeping live handles; return the best-scoring match.
    /// Returns `nil` when no element matches (never throws).
    private func locate(_ locator: AxLocator, maxDepth: Int = 24) -> (AXUIElement, AxElement)? {
        let target: AXUIElement
        if let p = locator.pid {
            target = AXUIElementCreateApplication(pid_t(p))
        } else {
            guard let app = NSWorkspace.shared.frontmostApplication else { return nil }
            target = AXUIElementCreateApplication(app.processIdentifier)
        }
        var best: (score: Double, handle: AXUIElement, meta: AxElement)?
        var count = 0
        func walk(_ ax: AXUIElement, depth: Int) {
            guard count < MAX_TREE_NODES else { return }
            count += 1
            let role = (axAttr(ax, kAXRoleAttribute) as? String) ?? "AXUnknown"
            let title = axAttr(ax, kAXTitleAttribute) as? String
            let bounds = boundsOf(ax)
            if let s = locatorScore(locator: locator, role: role, title: title, bounds: bounds) {
                if best == nil || s > best!.score {
                    var ownerPid: pid_t = 0
                    AXUIElementGetPid(ax, &ownerPid)
                    let rawValue = axAttr(ax, kAXValueAttribute)
                    let value: String?
                    if let rv = rawValue {
                        let s2 = "\(rv)"
                        value = s2.isEmpty ? nil : s2
                    } else {
                        value = nil
                    }
                    best = (s, ax, AxElement(
                        role: role, title: title, value: value,
                        bounds: bounds, pid: ownerPid, children: []
                    ))
                }
            }
            if depth < maxDepth {
                for child in (axAttr(ax, kAXChildrenAttribute) as? [AXUIElement] ?? []) {
                    walk(child, depth: depth + 1)
                }
            }
        }
        walk(target, depth: 0)
        return best.map { ($0.handle, $0.meta) }
    }

    private func collectByRole(_ el: AxElement, role: String) -> [AxElement] {
        var out: [AxElement] = []
        if el.role == role { out.append(el) }
        for child in el.children {
            out.append(contentsOf: collectByRole(child, role: role))
        }
        return out
    }

    private func buildElement(from ax: AXUIElement, depth: Int, maxDepth: Int, nodeCount: inout Int) -> AxElement? {
        guard nodeCount < MAX_TREE_NODES else { return nil }
        nodeCount += 1

        let role = (axAttr(ax, kAXRoleAttribute) as? String) ?? "AXUnknown"
        let title = axAttr(ax, kAXTitleAttribute) as? String
        let rawValue = axAttr(ax, kAXValueAttribute)
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
                buildElement(from: $0, depth: depth + 1, maxDepth: maxDepth, nodeCount: &nodeCount)
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
