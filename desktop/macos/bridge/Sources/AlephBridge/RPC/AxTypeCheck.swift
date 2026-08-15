import ApplicationServices
import Foundation

// MARK: - Runtime-checked bridges to Accessibility CFTypes
//
// `AXUIElementCopyAttributeValue` hands back an `AnyObject?` whose real type is
// only *documented*, never guaranteed: a misbehaving element can answer
// `kAXPositionAttribute` with something that is not an `AXValue`. Every call
// site that reads one of these attributes wants the same thing — check the
// type, and bail to `nil` rather than force-cast — because `as!` would trap and
// kill the helper subprocess, and a helper that dies mid-RPC is indistinguishable
// from a refused action to the bridge client.
//
// Three call sites used to spell that intent as `x as? AXValue`. It reads like a
// checked cast and is not one: a *conditional downcast to a CoreFoundation type
// always succeeds*, so the defence each of those comments described had never
// once run. Swift 6.4 promotes that to a hard error, which is how it surfaced —
// the diagnostic names the correct check in its own note ("did you mean to
// explicitly compare the CFTypeIDs?").
//
// `CFGetTypeID` is that check. It is the only one of the two that can answer
// "no", so it is the only one worth writing; the `as!` below is unconditional by
// construction, guarded by the comparison on the line above it.

/// `value` as an `AXValue`, or `nil` if it is some other CFType.
func asAXValue(_ value: AnyObject) -> AXValue? {
    guard CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    return (value as! AXValue)  // safe: type id compared on the line above
}

/// `value` as an `AXUIElement`, or `nil` if it is some other CFType.
func asAXUIElement(_ value: AnyObject) -> AXUIElement? {
    guard CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    return (value as! AXUIElement)  // safe: type id compared on the line above
}
