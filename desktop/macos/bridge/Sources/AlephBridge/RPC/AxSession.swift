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
///
/// Every affordance field is `Optional`, and Swift's synthesised `encode(to:)`
/// omits a `nil` Optional rather than writing `null`. That is the compatibility
/// contract with the Rust decoder: an absent field means "this helper could not
/// tell", never "no" — which is also what an older helper binary that predates
/// the affordances transmits (i.e. nothing at all).
struct AxElement: Codable {
    let role: String
    let title: String?
    let value: String?
    let bounds: Region?
    let pid: pid_t      // pid_t == Int32, serialises as JSON number
    /// Whether the element masks its content (a password field). Unlike the
    /// other affordances this is *always* emitted, because a secure field always
    /// reports its subrole and so its absence is itself definite. That also lets
    /// the Rust side use `secure != null` to detect an affordance-aware helper.
    let secure: Bool?
    /// `false` when the element is present but greyed out; absent when the
    /// element does not expose `AXEnabled` at all (containers, static text).
    let enabled: Bool?
    /// Whether `AXValue` accepts a write; absent when the element has no value.
    let settable: Bool?
    /// Raw AX action names, verbatim (`AXPress`, `AXShowMenu`, …). Emitted
    /// whenever AX answered, so an empty array means "no actions", not "unknown".
    let actions: [String]?
    let url: String?
    var children: [AxElement]
}

// Params decoded from the JSON-RPC request
struct QueryFocusedParams: Codable {
    /// Ask this process for its own focused element instead of asking the
    /// system which element is focused overall. Absent = system-wide.
    var pid: Int32?
}

struct QueryTreeParams: Codable {
    var pid: Int32?
    var max_depth: Int?     // snake_case matches Rust JSON field name
    var max_nodes: Int?
}

struct QueryByRoleParams: Codable {
    var role: String
    var pid: Int32?
    var max_nodes: Int?
}

/// What a budgeted walk spent, and whether it ran out.
struct WalkBudget {
    var nodeCount: Int = 0
    var truncated: Bool = false
}

// Response envelopes
struct QueryResult: Codable {
    let element: AxElement?
    let node_count: Int
    let truncated: Bool

    init(element: AxElement?, budget: WalkBudget = WalkBudget()) {
        self.element = element
        self.node_count = budget.nodeCount
        self.truncated = budget.truncated
    }
}

struct QueryListResult: Codable {
    let elements: [AxElement]
    let node_count: Int
    let truncated: Bool

    init(elements: [AxElement], budget: WalkBudget = WalkBudget()) {
        self.elements = elements
        self.node_count = budget.nodeCount
        self.truncated = budget.truncated
    }
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

/// AX reports a password field as role `AXTextField` carrying the subrole
/// `AXSecureTextField`; the role alone cannot tell it apart from a plain text
/// field. Pure — testable without live AX handles.
func isSecureSubrole(_ subrole: String?) -> Bool {
    subrole == (kAXSecureTextFieldSubrole as String)
}

/// Whether an application's AX tree is still an unpopulated shell.
///
/// A Chromium/Electron app whose accessibility is switched off answers with a
/// bare shell: the AXApplication has no children at all, or exactly one window
/// that itself has no children. A healthy single-window Cocoa app also has one
/// root child, so the root count alone would flag it too — the grandchild count
/// is what separates the two, and keeping that check is what stops the
/// `AXEnhancedUserInterface` escalation from firing on healthy apps.
///
/// Pure tree shape: never inspects a role, title or value. Pass `nil` for
/// `firstChildChildCount` when there is no first child.
func isShellTree(rootChildCount: Int, firstChildChildCount: Int?) -> Bool {
    guard rootChildCount <= 1 else { return false }
    guard let grandchildren = firstChildChildCount else { return true }
    return grandchildren == 0
}

// MARK: - AxQuerier actor

/// The affordance half of an `AxElement`, read off a live AX element.
/// `nil` carries "AX did not answer" through to an omitted JSON field.
private struct AxAffordances {
    let secure: Bool
    let enabled: Bool?
    let settable: Bool?
    let actions: [String]?
    let url: String?
}

/// Serializes AX API calls and builds element trees.
///
/// All methods run on the actor's executor so that AX API calls
/// (which must not race) are naturally serialised. The one-off unlock in
/// `appElement(pid:)` is the only suspension point, and it guards its own state
/// by claiming the pid before it settles, so a reentrant call cannot double-write.
actor AxQuerier {

    /// Fallback node budget, used only when the caller sent none.
    ///
    /// The real budget is the protocol's (`ax::DEFAULT_MAX_NODES`) and arrives in
    /// `max_nodes`. This constant exists for an older client that does not send
    /// the field, and matches the protocol's value on purpose — two numbers that
    /// mean the same thing must not be allowed to differ.
    private static let defaultMaxNodes = 1_500

    /// How long any single AX call may wait on the target application.
    ///
    /// Accessibility calls are synchronous IPC into another process, and the
    /// system default lets them wait a long time. That matters here more than it
    /// looks: this type is an `actor`, so every AX operation in the helper is
    /// serialised behind whichever one is currently blocked — one beachballing
    /// app therefore stalls `ax.query_focused` for *every* app, and that call
    /// runs before every keystroke the agent types. Bounding it turns "the
    /// accessibility layer has stopped working" into "that one app did not
    /// answer", which is both true and recoverable.
    ///
    /// Comfortably inside the client-side deadline for the AX namespace, so a
    /// stuck element surfaces as a partial tree rather than as a dead call.
    private static let axMessagingTimeout: Float = 2.0

    /// The private attribute Chromium honours to switch its accessibility tree
    /// on. Undocumented, hence no `kAX…` constant exists for it.
    private static let manualAccessibilityAttribute = "AXManualAccessibility"

    /// The VoiceOver-era AppKit global. It also switches Chromium's tree on, but
    /// it is app-wide, persists until the target app restarts, and makes AppKit
    /// window move/resize slow — window managers (yabai, Rectangle) clear it on
    /// purpose. So it is a last resort, never the opening move.
    private static let enhancedUserInterfaceAttribute = "AXEnhancedUserInterface"

    /// Chromium builds its tree asynchronously once the unlock write lands. Wait
    /// one short beat before concluding the write had no effect, so that a merely
    /// slow app is never escalated, and so that the caller's walk (which happens
    /// straight after) sees the populated tree on this very call rather than the
    /// next one.
    private static let unlockSettleNanos: UInt64 = 120_000_000

    /// pids already offered the unlock. The write is idempotent inside the target
    /// app, so one attempt per pid per bridge lifetime is enough.
    private var unlockedPids: Set<pid_t> = []

    // MARK: Public interface

    /// The focused element — of one application, or of the system.
    ///
    /// With a `pid` this asks *that application* for its `AXFocusedUIElement`,
    /// which is a different question from the system-wide one and the only one
    /// worth asking on the targeted input rail: that rail delivers keystrokes
    /// into a named process without bringing it forward, so the system-focused
    /// element usually belongs to some other app entirely. Reading it there meant
    /// the `type_text` focus gate — including its hard refusal to type into a
    /// password field — was inspecting a window the keystrokes were never going
    /// to reach.
    ///
    /// Without a `pid` the system-wide element is still the right answer: that is
    /// exactly where the global rail's keystrokes go.
    func queryFocused(pid: pid_t?) async -> AxElement? {
        let root: AXUIElement
        if let pid {
            // Route through `appElement` so a Chromium app gets its one-time
            // accessibility unlock here too: without it such an app reports no
            // focused element at all, and "no focus" is the fail-open answer.
            guard let app = await appElement(pid: pid) else { return nil }
            root = app
        } else {
            root = withTimeout(AXUIElementCreateSystemWide())
        }
        guard let el = axAttr(root, kAXFocusedUIElementAttribute) else { return nil }
        // `kAXFocusedUIElementAttribute` is documented to return an
        // `AXUIElementRef`, but a misbehaving element can return a different
        // CFType — and `as!` would crash the helper, which the bridge client
        // reads as a refused action. Conditional cast + bail preserves the
        // "no focus" answer (the open `guard let el = axAttr(...)` already
        // named it the fail-open case in its comment).
        guard let axEl = el as? AXUIElement else { return nil }
        var budget = WalkBudget()
        return buildElement(
            from: withTimeout(axEl),
            depth: 0, maxDepth: 2, maxNodes: Self.defaultMaxNodes, budget: &budget
        )
    }

    func queryTree(pid: pid_t?, maxDepth: Int, maxNodes: Int) async -> (AxElement?, WalkBudget) {
        var budget = WalkBudget()
        guard let target = await appElement(pid: pid) else { return (nil, budget) }
        let el = buildElement(
            from: target, depth: 0, maxDepth: maxDepth, maxNodes: maxNodes, budget: &budget
        )
        return (el, budget)
    }

    func queryByRole(role: String, pid: pid_t?, maxNodes: Int) async -> ([AxElement], WalkBudget) {
        let (root, budget) = await queryTree(pid: pid, maxDepth: 8, maxNodes: maxNodes)
        guard let root else { return ([], budget) }
        return (collectByRole(root, role: role), budget)
    }

    func setValue(_ params: SetValueParams) async throws -> AxActionResult {
        guard let (handle, meta) = await locate(params.locator) else {
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
                    // The comparison happens here, but the content must not leave:
                    // a preview of a secure field is a password on its way into the
                    // model context, the transcript and memory. The mismatch is still
                    // reported — only the evidence is withheld.
                    actual_preview: meta.secure == true ? nil : String(a.prefix(200))
                )
        } else {
            verification = AxVerification(state: "unverified", reason: "value_unreadable", actual_preview: nil)
        }
        return AxActionResult(performed: true, path: "accessibility", matched: meta, verification: verification)
    }

    func performAction(_ params: PerformActionParams) async throws -> AxActionResult {
        guard let (handle, meta) = await locate(params.locator) else {
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

    // MARK: Accessibility unlock

    /// Resolve the application element for `pid` (or the frontmost app), and on
    /// the first sighting of that pid switch its accessibility tree on.
    ///
    /// Chromium-based apps — Chrome, VS Code, Slack, Discord, Notion, Obsidian,
    /// Figma, i.e. exactly the apps worth driving — do not expose their content
    /// subtree until a client writes `AXManualAccessibility` on the application
    /// element. Skip the write and the walk returns an AXApplication with an
    /// empty shell of a subtree *and no error*, so the failure is silent rather
    /// than loud: `ax.query_tree` reports success while returning nothing, and
    /// `locate` matches nothing, which surfaces to the model as a bogus -32602.
    ///
    /// Returns `nil` only when no pid was given and there is no frontmost app.
    private func appElement(pid: pid_t?) async -> AXUIElement? {
        let target: pid_t
        if let p = pid {
            target = p
        } else {
            guard let app = NSWorkspace.shared.frontmostApplication else { return nil }
            target = app.processIdentifier
        }
        let ax = withTimeout(AXUIElementCreateApplication(target))
        // `inserted` is false on every later call for this pid, so the writes below
        // run exactly once per app — including for the concurrent caller that
        // arrives while the first one is still settling.
        guard unlockedPids.insert(target).inserted else { return ax }
        await unlock(ax)
        return ax
    }

    /// Best-effort accessibility unlock. Run once per pid, before the first walk.
    private func unlock(_ app: AXUIElement) async {
        // An app that does not implement the attribute simply rejects the write,
        // so the result is discarded on purpose: there is nothing to recover from
        // and every non-Chromium app lands here.
        _ = AXUIElementSetAttributeValue(
            app, Self.manualAccessibilityAttribute as CFString, kCFBooleanTrue
        )
        guard isShell(app) else { return }
        try? await Task.sleep(nanoseconds: Self.unlockSettleNanos)
        // Still a shell: the attribute was not understood. Escalate to the blunt
        // global — accepting its app-wide slow-window-resize side effect, which is
        // why it is gated on tree shape instead of written unconditionally.
        guard isShell(app) else { return }
        _ = AXUIElementSetAttributeValue(
            app, Self.enhancedUserInterfaceAttribute as CFString, kCFBooleanTrue
        )
        try? await Task.sleep(nanoseconds: Self.unlockSettleNanos)
    }

    /// Two AX reads, no content inspected — see `isShellTree`.
    private func isShell(_ app: AXUIElement) -> Bool {
        let children = axAttr(app, kAXChildrenAttribute) as? [AXUIElement] ?? []
        let grandchildren = children.first.map {
            (axAttr($0, kAXChildrenAttribute) as? [AXUIElement] ?? []).count
        }
        return isShellTree(rootChildCount: children.count, firstChildChildCount: grandchildren)
    }

    // MARK: Private helpers

    /// Walk the AX tree keeping live handles; return the best-scoring match.
    /// Returns `nil` when no element matches (never throws).
    private func locate(_ locator: AxLocator, maxDepth: Int = 24) async -> (AXUIElement, AxElement)? {
        guard let target = await appElement(pid: locator.pid.map { pid_t($0) }) else { return nil }
        var best: (score: Double, handle: AXUIElement, meta: AxElement)?
        var count = 0
        func walk(_ ax: AXUIElement, depth: Int) {
            guard count < Self.defaultMaxNodes else { return }
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
                    // Only the element we actually adopt pays for the affordance
                    // reads — the walk itself stays at four AX calls per node.
                    let extra = affordances(of: ax, hasValue: rawValue != nil)
                    best = (s, ax, AxElement(
                        role: role, title: title, value: value,
                        bounds: bounds, pid: ownerPid,
                        secure: extra.secure, enabled: extra.enabled,
                        settable: extra.settable, actions: extra.actions, url: extra.url,
                        children: []
                    ))
                }
            }
            if depth < maxDepth {
                for child in (axAttr(ax, kAXChildrenAttribute) as? [AXUIElement] ?? []) {
                    walk(withTimeout(child), depth: depth + 1)
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

    /// Materialise one node and, budget permitting, its subtree.
    ///
    /// Returning `nil` on an exhausted budget is what prunes the walk, and
    /// `budget.truncated` is what makes that visible: a caller handed a silently
    /// clipped tree concludes the control it is hunting for does not exist.
    private func buildElement(
        from ax: AXUIElement, depth: Int, maxDepth: Int, maxNodes: Int, budget: inout WalkBudget
    ) -> AxElement? {
        guard budget.nodeCount < maxNodes else {
            budget.truncated = true
            return nil
        }
        budget.nodeCount += 1

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
        let extra = affordances(of: ax, hasValue: rawValue != nil)

        var children: [AxElement] = []
        if depth < maxDepth {
            let rawChildren = axAttr(ax, kAXChildrenAttribute) as? [AXUIElement] ?? []
            children = rawChildren.compactMap {
                buildElement(
                    from: withTimeout($0), depth: depth + 1, maxDepth: maxDepth,
                    maxNodes: maxNodes, budget: &budget
                )
            }
        }
        return AxElement(
            role: role,
            title: title,
            value: value,
            bounds: bounds,
            pid: ownerPid,
            secure: extra.secure,
            enabled: extra.enabled,
            settable: extra.settable,
            actions: extra.actions,
            url: extra.url,
            children: children
        )
    }

    /// Read the affordances an AX element already advertises and that the bridge
    /// used to throw away: what it can be asked to do, whether it may be asked at
    /// all, and whether its value is a secret.
    ///
    /// Action names go out raw (`AXPress`, `AXShowMenu`, `AXRaise`, …) and
    /// unfiltered. `ax.perform_action` is a verbatim pass-through to
    /// `AXUIElementPerformAction`, so an app-specific action already works
    /// end-to-end — the model simply could not see that it existed and was left
    /// guessing among the handful of names in the tool description. Prettifying
    /// or renaming them here would only put string semantics in the limb; the
    /// model reads AX names as-is.
    ///
    /// Pass `hasValue: false` when the element exposes no `AXValue`: asking
    /// whether a non-existent attribute is settable just costs an AX round trip
    /// to be told it is unsupported.
    private func affordances(of ax: AXUIElement, hasValue: Bool) -> AxAffordances {
        var settable: Bool?
        if hasValue {
            var flag: DarwinBoolean = false
            if AXUIElementIsAttributeSettable(ax, kAXValueAttribute as CFString, &flag) == .success {
                settable = flag.boolValue
            }
        }

        var actions: [String]?
        var names: CFArray?
        if AXUIElementCopyActionNames(ax, &names) == .success {
            actions = (names as? [String]) ?? []
        }

        return AxAffordances(
            secure: isSecureSubrole(axAttr(ax, kAXSubroleAttribute) as? String),
            enabled: axAttr(ax, kAXEnabledAttribute) as? Bool,
            settable: settable,
            actions: actions,
            url: urlOf(ax)
        )
    }

    private func urlOf(_ ax: AXUIElement) -> String? {
        guard let raw = axAttr(ax, kAXURLAttribute) else { return nil }
        if let u = raw as? URL { return u.absoluteString }
        return raw as? String
    }

    /// Bound how long calls on `element` may block on its owning application.
    ///
    /// `AXUIElementSetMessagingTimeout` is per-element and is **not** inherited
    /// by elements copied out of it, which is why this is applied at every point
    /// a new handle enters the walk (the app element, the system-wide element,
    /// and each child) rather than once at the root. Missing one of those puts
    /// that branch back on the system default.
    ///
    /// Returns its argument so it can be wrapped inline at the point of use —
    /// the shape that makes "did we bound this handle?" answerable by reading
    /// one line instead of tracing where it came from.
    @discardableResult
    private func withTimeout(_ element: AXUIElement) -> AXUIElement {
        AXUIElementSetMessagingTimeout(element, Self.axMessagingTimeout)
        return element
    }

    private func axAttr(_ ax: AXUIElement, _ name: String) -> AnyObject? {
        var v: AnyObject?
        let err = AXUIElementCopyAttributeValue(ax, name as CFString, &v)
        return err == .success ? v : nil
    }

    private func boundsOf(_ ax: AXUIElement) -> Region? {
        var posVal: AnyObject?
        var sizeVal: AnyObject?
        // kAXPositionAttribute/kAXSizeAttribute are documented AXValue
        // returns, but a misbehaving element can hand back a different CFType
        // and `as!` would crash the helper subprocess — which the bridge
        // client reads as a refused action (silent rung failure). Conditional
        // cast + bail to `nil` lets the caller report "no bounds" and the
        // rung above retry with a different element.
        guard AXUIElementCopyAttributeValue(ax, kAXPositionAttribute as CFString, &posVal) == .success,
              AXUIElementCopyAttributeValue(ax, kAXSizeAttribute as CFString, &sizeVal) == .success,
              let pv = posVal, let sv = sizeVal,
              let pvValue = pv as? AXValue, let svValue = sv as? AXValue
        else { return nil }
        var point = CGPoint.zero
        var size  = CGSize.zero
        guard AXValueGetValue(pvValue, .cgPoint, &point),
              AXValueGetValue(svValue, .cgSize, &size)
        else { return nil }
        return Region(
            x: Double(point.x),
            y: Double(point.y),
            width: Double(size.width),
            height: Double(size.height)
        )
    }
}
