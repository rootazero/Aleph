import ApplicationServices
import CoreGraphics
import Foundation

// MARK: - Delivery honesty (read this before touching the ladder)
//
// The background rail here delivers with `CGEvent.postToPid(_:)`, never with
// `CGEvent.post(tap:)`:
//
//   post(tap: .cghidEventTap)  → the GLOBAL HID stream. The user's physical
//                                cursor is dragged to the point, and only the
//                                frontmost app can receive the event.
//   postToPid(pid)             → straight into one process's event queue. The
//                                user's cursor never moves and the target app
//                                need not be frontmost.
//
// The catch, verified on macOS 27 and the reason the mouse methods below refuse:
// `postToPid` delivers KEYBOARD events but NOT located mouse events. A keyboard
// event routes to the process's key-window first responder — no location, no
// window-server involvement — so `type_text` / `key_combo` land. A mouse event
// carries a screen point, and the window server (which `postToPid` bypasses) is
// what associates that point with a window; without it the app receives the
// event but never routes it to a view, so it does nothing. The identical mouse
// event on the global tap DOES act — but that moves the user's cursor.
//
// So the background rail on macOS is: keyboard synthesis + the AX click ladder
// (pressing an element that advertises the action, which needs no coordinate
// delivery at all). Everything else — a coordinate click with no AX rung, a
// scroll, a drag, a hover, a raw mouse press — has no background delivery, and
// those methods throw `backgroundMouseUndeliverable` rather than post an event
// the app will never act on and report success. The lie they refuse to tell is
// exactly what the e2e caught.
//
// `delivery: "targeted"` in a result therefore means "delivered to this process,
// cursor untouched, and the app can act on it" — keyboard, or an AX press. It is
// never returned for a mouse event that was merely posted-and-hoped.
//
// Delivery is never silently downgraded to the global tap here: a request that
// cannot run in the background is an error, not a cursor-moving fallback.
// Whether to fall back to the visible cursor is a ROUTING POLICY that lives in
// Rust (`allow_global_pointer`), where the model can be told which rail ran (A2
// — the limb hands the error up, it never picks the recovery strategy).

// MARK: - Wire-format types
//
// Mirror `aleph_protocol::desktop_bridge::methods::input`. Field names are the
// Rust JSON keys verbatim (hence `click_count`, `start_x`, … in snake_case).

/// Mirrors `input::MouseButton` — serialises lowercase.
enum MouseButton: String, Codable {
    case left
    case right
    case middle
}

/// Mirrors `input::PressAction` — serialises lowercase.
enum PressAction: String, Codable {
    case press
    case release
    case click
}

/// Params for `input.click` **and** `input.double_click`.
/// `input.double_click` ignores `click_count` and always delivers click count 2.
struct ClickParams: Codable {
    var x: Double
    var y: Double
    var button: MouseButton
    var pid: Int32?
    var click_count: UInt32?
}

/// Result for `input.click` / `input.double_click`.
///
/// `ok` + `delivery` are the frozen contract (`input::ClickResult`). `path` and
/// `matched` are ADDITIVE: the Rust struct does not declare them yet, and serde
/// ignores unknown fields, so emitting them is wire-safe today and readable by
/// the router the moment it wants them. They exist because a click that was
/// satisfied by pressing an AX element is a materially different act from a
/// synthesized mouse event at a coordinate, and the model must be able to see
/// WHICH ONE happened and WHAT was pressed.
struct ClickResult: Codable {
    let ok: Bool
    /// `input::DELIVERY_TARGETED` — this rail never posts to the global tap.
    let delivery: String
    /// `"accessibility"` (an AX action was performed) or `"synthetic"` (a mouse
    /// event was posted). Same vocabulary as `ax.perform_action`'s `path`.
    let path: String
    /// The element actually acted on, when `path == "accessibility"`.
    let matched: AxElement?
}

struct TypeTextParams: Codable {
    var text: String
    var pid: Int32?
}

struct TypeTextResult: Codable {
    let ok: Bool
    let delivery: String
}

struct KeyComboParams: Codable {
    var modifiers: [String]
    var key: String
    var pid: Int32?
}

struct KeyComboResult: Codable {
    let ok: Bool
    let delivery: String
}

/// Params for `input.key_button` — hold a chord down, or let it back up.
///
/// Distinct from `input.key_combo` because the press outlives the call: the
/// release arrives in a LATER request, and it has to reach the same process.
struct KeyButtonParams: Codable {
    var keys: [String]
    var action: PressAction
    var pid: Int32?
}

struct KeyButtonResult: Codable {
    let ok: Bool
    let delivery: String
}

struct ScrollParams: Codable {
    var direction: String
    var amount: Int32
    var pid: Int32?
    /// Where the scroll is aimed, in global screen points.
    ///
    /// The global rail scrolls at the real cursor, so it may omit these. The
    /// targeted rail may not: the cursor never moves, so the event's location
    /// field is the ONLY thing the app can route the scroll by.
    var x: Double?
    var y: Double?
}

struct ScrollResult: Codable {
    let ok: Bool
    let delivery: String
}

struct DragParams: Codable {
    var start_x: Double
    var start_y: Double
    var end_x: Double
    var end_y: Double
    var duration_ms: UInt64?
    var pid: Int32?
}

struct DragResult: Codable {
    let ok: Bool
    let delivery: String
}

struct HoverParams: Codable {
    var x: Double
    var y: Double
    var pid: Int32?
}

struct HoverResult: Codable {
    let ok: Bool
    let delivery: String
}

struct MouseButtonParams: Codable {
    var x: Double
    var y: Double
    var button: MouseButton
    var action: PressAction
    var pid: Int32?
}

struct MouseButtonResult: Codable {
    let ok: Bool
    let delivery: String
}

struct CursorPositionResult: Codable {
    let x: Double
    let y: Double
}

/// Mirrors `input::DELIVERY_TARGETED`.
///
/// Its counterpart `DELIVERY_GLOBAL` deliberately has no Swift constant: this
/// file has no code path that can produce it. Every event here goes out via
/// `postToPid`, and a request that cannot be targeted is rejected rather than
/// downgraded — so "global" is a value only the Rust caller can ever report,
/// about a rail only the Rust caller owns.
let DELIVERY_TARGETED = "targeted"

// MARK: - Pure helpers (no live event state — unit-testable)

/// Split `text` into chunks of at most `maxUTF16` UTF-16 code units,
/// **never splitting a grapheme cluster**.
///
/// `keyboardSetUnicodeString` takes a UTF-16 buffer, and macOS drops
/// unreasonably long ones, hence the chunking. Iterating `Character` (grapheme
/// clusters) rather than `unicodeScalars` is what keeps an emoji, a flag, or a
/// combining accent from being cut in half and delivered as mojibake.
///
/// A single grapheme longer than `maxUTF16` (a pathological ZWJ chain) is kept
/// whole in its own oversized chunk: half a grapheme is always wrong, an
/// oversized one is merely unlikely.
func chunkForUnicodeEvents(_ text: String, maxUTF16: Int = 64) -> [String] {
    var chunks: [String] = []
    var current = ""
    var currentLen = 0
    for ch in text {
        let len = String(ch).utf16.count
        if currentLen > 0, currentLen + len > maxUTF16 {
            chunks.append(current)
            current = ""
            currentLen = 0
        }
        current.append(ch)
        currentLen += len
    }
    if !current.isEmpty { chunks.append(current) }
    return chunks
}

/// ANSI virtual key code for a key name, or `nil` when the name is unknown.
///
/// The vocabulary is deliberately identical to the global rail's
/// (`desktop/shared/src/action/key_parse.rs`): a key name must mean the same
/// thing on both rails, or the model would have to know which rail it is on.
///
/// Single characters are looked up case-insensitively in a fixed ANSI table
/// rather than through the user's active layout: a key COMBO is positional
/// (⌘Z is the key where Z sits), and reading the live layout would make the same
/// request behave differently on a Dvorak or AZERTY machine. Text goes through
/// `type_text`, which is layout-independent by construction (Unicode payload,
/// no key code at all).
func virtualKeyCode(for name: String) -> CGKeyCode? {
    switch name.lowercased() {
    // Letters
    case "a": return 0
    case "b": return 11
    case "c": return 8
    case "d": return 2
    case "e": return 14
    case "f": return 3
    case "g": return 5
    case "h": return 4
    case "i": return 34
    case "j": return 38
    case "k": return 40
    case "l": return 37
    case "m": return 46
    case "n": return 45
    case "o": return 31
    case "p": return 35
    case "q": return 12
    case "r": return 15
    case "s": return 1
    case "t": return 17
    case "u": return 32
    case "v": return 9
    case "w": return 13
    case "x": return 7
    case "y": return 16
    case "z": return 6
    // Digits
    case "0": return 29
    case "1": return 18
    case "2": return 19
    case "3": return 20
    case "4": return 21
    case "5": return 23
    case "6": return 22
    case "7": return 26
    case "8": return 28
    case "9": return 25
    // Unshifted ANSI punctuation
    case "-": return 27
    case "=": return 24
    case "[": return 33
    case "]": return 30
    case "\\": return 42
    case ";": return 41
    case "'": return 39
    case ",": return 43
    case ".": return 47
    case "/": return 44
    case "`": return 50
    // Named keys — same aliases the global rail accepts
    case "space": return 49
    case "return", "enter": return 36
    case "tab": return 48
    case "escape", "esc": return 53
    case "backspace": return 51
    case "delete", "del": return 117
    case "up", "uparrow": return 126
    case "down", "downarrow": return 125
    case "left", "leftarrow": return 123
    case "right", "rightarrow": return 124
    case "home": return 115
    case "end": return 119
    case "pageup": return 116
    case "pagedown": return 121
    case "f1": return 122
    case "f2": return 120
    case "f3": return 99
    case "f4": return 118
    case "f5": return 96
    case "f6": return 97
    case "f7": return 98
    case "f8": return 100
    case "f9": return 101
    case "f10": return 109
    case "f11": return 103
    case "f12": return 111
    default: return nil
    }
}

/// The flag a modifier name raises and the key code that raises it, or `nil` for
/// an unknown name. Aliases match the global rail's `parse_modifier`.
func modifierKey(for name: String) -> (flag: CGEventFlags, code: CGKeyCode)? {
    switch name.lowercased() {
    case "meta", "command", "cmd", "super", "win": return (.maskCommand, 55)
    case "shift": return (.maskShift, 56)
    case "control", "ctrl": return (.maskControl, 59)
    case "alt", "option": return (.maskAlternate, 58)
    default: return nil
    }
}

/// Whether a rectangle contains a point (edges inclusive).
///
/// Load-bearing for the AX ladder, not cosmetic: the approval gate classifies
/// the literal `click(x, y)` the model asked for, so an AX press is only allowed
/// to stand in for that click if it lands on an element the point is actually
/// inside of. See `AxClickLadder`.
func boundsContain(_ bounds: Region, x: Double, y: Double) -> Bool {
    x >= bounds.x && x <= bounds.x + bounds.width
        && y >= bounds.y && y <= bounds.y + bounds.height
}

/// AX actions that stand in for a mouse click of each button, in priority order.
///
/// A fixed role/action table, not a semantic read of the element: the ladder
/// asks "does this element advertise the action a click performs?", never "does
/// this element's TITLE look like something worth clicking?" (R7/P8 — the
/// reference implementation's keyword screen of element titles is a semantic
/// judgement made on the model's behalf and is deliberately not ported).
///
/// Middle-click has no AX equivalent, so it yields no rung and always
/// synthesizes.
func axActionsStandingInForClick(_ button: MouseButton) -> [String] {
    switch button {
    case .left: return ["AXPress", "AXConfirm", "AXOpen"]
    case .right: return ["AXShowMenu"]
    case .middle: return []
    }
}

/// Pick the element an AX click should act on, from candidates already filtered
/// to "contains the point" and "advertises a stand-in action".
///
/// Ranking is purely geometric and content-blind: the SMALLEST element wins,
/// because at a given point the smallest box containing it is the most specific
/// control there (the button, not the toolbar that holds it, not the window that
/// holds the toolbar). `depth` breaks exact-area ties toward the shallower node
/// so the choice is deterministic rather than dependent on AX child order.
///
/// Pure so the ranking is testable without a live AX tree.
func rankClickCandidates(_ candidates: [(area: Double, depth: Int, index: Int)]) -> Int? {
    candidates.min {
        if $0.area != $1.area { return $0.area < $1.area }
        return $0.depth < $1.depth
    }?.index
}

// MARK: - InputSession actor

/// Serialises every synthesized-input call and owns the one event source they
/// are all built from.
///
/// The rail that survives on macOS is keyboard synthesis (`type_text`,
/// `key_combo`) plus the AX click ladder. Synthesized *mouse* events cannot be
/// delivered to a background process here at all — a located event handed to
/// `postToPid` is never routed to a window — so those methods refuse rather than
/// pretend (see `backgroundMouseUndeliverable`). The event source is a single
/// long-lived `.combinedSessionState` instance so that a modifier the user is
/// holding, or a key held across calls by `key_button`, composes into the next
/// synthesized keystroke instead of being born empty and dying on return.
actor InputSession {

    /// How deep below the hit element the AX ladder looks for a more specific
    /// actionable control. Three levels covers button-inside-cell-inside-row
    /// without turning a click into a tree walk.
    private static let axLadderDepth = 3
    /// Guard against a pathological AX subtree turning one click into thousands
    /// of AX round trips.
    private static let axLadderMaxNodes = 400

    private let source = CGEventSource(stateID: .combinedSessionState)

    /// Modifiers this rail is currently holding down, per target process.
    ///
    /// The one piece of cross-call state in this file, and it is unavoidable: a
    /// modifier held by `input.key_button` stays down across requests, and the
    /// events sent in between have to carry it. `flags` is how an app reads a
    /// chord off a key event, so without this a caller that pressed ⌘ and then
    /// asked for `key_combo(key: "a")` would get a bare "a" — the key-down for ⌘
    /// was delivered, but nothing after it said ⌘ was down. It is keyed by pid
    /// because the rail is, and it only ever holds MODIFIERS: an ordinary key
    /// left down has no representation in `flags` and no effect on other events.
    private var heldModifiers: [pid_t: CGEventFlags] = [:]

    // MARK: Actions

    func click(_ params: ClickParams) throws -> ClickResult {
        let pid = try requirePid(params.pid, method: "input.click")
        let count = max(1, Int(params.click_count ?? 1))
        return try performClick(pid: pid, x: params.x, y: params.y, button: params.button, count: count)
    }

    /// `input.double_click` reuses `ClickParams` and IGNORES `click_count`: the
    /// method name is the request.
    func doubleClick(_ params: ClickParams) throws -> ClickResult {
        let pid = try requirePid(params.pid, method: "input.double_click")
        return try performClick(pid: pid, x: params.x, y: params.y, button: params.button, count: 2)
    }

    func typeText(_ params: TypeTextParams) throws -> TypeTextResult {
        let pid = try requirePid(params.pid, method: "input.type_text")
        let source = try eventSource()

        for chunk in chunkForUnicodeEvents(params.text) {
            let utf16 = Array(chunk.utf16)
            guard let down = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true),
                  let up = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: false)
            else {
                throw RpcError(code: -32_603, message: "CGEvent(keyboardEventSource:) returned nil", data: nil)
            }
            // The Unicode payload IS the text — no key code, no layout lookup, so
            // this types the same characters on a Dvorak or Cyrillic layout.
            // Flags are cleared for exactly that reason: the session-state source
            // reports a modifier the USER happens to be holding, and an inherited
            // ⌘ would turn typed text into a stream of keyboard shortcuts.
            down.flags = []
            up.flags = []
            utf16.withUnsafeBufferPointer { buf in
                down.keyboardSetUnicodeString(stringLength: buf.count, unicodeString: buf.baseAddress)
                up.keyboardSetUnicodeString(stringLength: buf.count, unicodeString: buf.baseAddress)
            }
            down.postToPid(pid)
            up.postToPid(pid)
        }
        return TypeTextResult(ok: true, delivery: DELIVERY_TARGETED)
    }

    func keyCombo(_ params: KeyComboParams) throws -> KeyComboResult {
        let pid = try requirePid(params.pid, method: "input.key_combo")
        let source = try eventSource()

        var mods: [(flag: CGEventFlags, code: CGKeyCode)] = []
        for name in params.modifiers {
            guard let m = modifierKey(for: name) else {
                throw RpcError(
                    code: -32_602,
                    message: "unknown modifier: '\(name)'. Expected meta/command/cmd, shift, control/ctrl, alt/option",
                    data: nil
                )
            }
            mods.append(m)
        }
        guard let keyCode = virtualKeyCode(for: params.key) else {
            throw RpcError(
                code: -32_602,
                message: "unknown key: '\(params.key)'. Use a single ANSI character or a named key "
                    + "(space, return, tab, escape, backspace, delete, arrows, home, end, pageup, pagedown, f1-f12)",
                data: nil
            )
        }

        // Press the modifiers, then the key, then release in reverse. Apps read
        // the chord off `flags` on the key-down, but a modifier that never went
        // down is a modifier the app's own state tracking never saw — so the
        // flags are set AND the key-downs are posted.
        //
        // Seeded with whatever `input.key_button` is holding for this pid: a
        // caller that pressed ⌘ and then asks for "a" means ⌘A, and the flags on
        // this event are the only place that can be said.
        var flags: CGEventFlags = heldModifiers[pid] ?? []
        for mod in mods {
            flags.insert(mod.flag)
            try postKey(source: source, code: mod.code, down: true, flags: flags, pid: pid)
        }
        try postKey(source: source, code: keyCode, down: true, flags: flags, pid: pid)
        try postKey(source: source, code: keyCode, down: false, flags: flags, pid: pid)
        for mod in mods.reversed() {
            flags.remove(mod.flag)
            try postKey(source: source, code: mod.code, down: false, flags: flags, pid: pid)
        }
        return KeyComboResult(ok: true, delivery: DELIVERY_TARGETED)
    }

    /// Hold a chord down, or let it back up, in one process.
    ///
    /// Order mirrors the global rail (`desktop/shared/src/action/input.rs`):
    /// press forward, release in reverse, `click` does both. That ordering is
    /// not cosmetic — a chord released left-to-right hands the app a window in
    /// which the modifier is up but the key it modified is still down.
    func keyButton(_ params: KeyButtonParams) throws -> KeyButtonResult {
        let pid = try requirePid(params.pid, method: "input.key_button")
        guard !params.keys.isEmpty else {
            throw RpcError(code: -32_602, message: "input.key_button requires at least one key", data: nil)
        }
        let source = try eventSource()

        // A key is either a modifier (contributes a flag) or an ordinary key.
        // Resolve every one of them BEFORE posting anything: a chord that fails
        // halfway leaves keys physically down that nobody knows to release.
        enum Chorded {
            case modifier(flag: CGEventFlags, code: CGKeyCode)
            case plain(code: CGKeyCode)
        }
        let chord: [Chorded] = try params.keys.map { name in
            if let mod = modifierKey(for: name) {
                return .modifier(flag: mod.flag, code: mod.code)
            }
            guard let code = virtualKeyCode(for: name) else {
                throw RpcError(
                    code: -32_602,
                    message: "unknown key: '\(name)'. Use a modifier (meta/command/cmd, shift, control/ctrl, "
                        + "alt/option), a single ANSI character, or a named key (space, return, tab, escape, "
                        + "backspace, delete, arrows, home, end, pageup, pagedown, f1-f12)",
                    data: nil
                )
            }
            return .plain(code: code)
        }

        var flags: CGEventFlags = heldModifiers[pid] ?? []

        func pressForward() throws {
            for item in chord {
                switch item {
                case .modifier(let flag, let code):
                    flags.insert(flag)
                    try postKey(source: source, code: code, down: true, flags: flags, pid: pid)
                case .plain(let code):
                    try postKey(source: source, code: code, down: true, flags: flags, pid: pid)
                }
            }
        }
        func releaseReversed() throws {
            for item in chord.reversed() {
                switch item {
                case .modifier(let flag, let code):
                    flags.remove(flag)
                    try postKey(source: source, code: code, down: false, flags: flags, pid: pid)
                case .plain(let code):
                    try postKey(source: source, code: code, down: false, flags: flags, pid: pid)
                }
            }
        }

        switch params.action {
        case .press:
            try pressForward()
        case .release:
            try releaseReversed()
        case .click:
            try pressForward()
            try releaseReversed()
        }

        // Record what is still down so later events on this rail carry it.
        // Empty entries are dropped rather than kept as `[]`, so the map holds
        // only processes this rail is actually holding something in.
        if flags.isEmpty {
            heldModifiers.removeValue(forKey: pid)
        } else {
            heldModifiers[pid] = flags
        }
        return KeyButtonResult(ok: true, delivery: DELIVERY_TARGETED)
    }

    func scroll(_ params: ScrollParams) throws -> ScrollResult {
        _ = try requirePid(params.pid, method: "input.scroll")
        throw backgroundMouseUndeliverable(
            "A background scroll",
            alternative: "Scroll with the keyboard instead — `key_combo` with PageDown / PageUp / "
                + "Down / Up / Space reaches the focused view in the background."
        )
    }

    func drag(_ params: DragParams) throws -> DragResult {
        _ = try requirePid(params.pid, method: "input.drag")
        throw backgroundMouseUndeliverable(
            "A background drag",
            alternative: "A drag has no background equivalent on macOS — the app tracks it through "
                + "mouse motion the window server never associates with a synthesized event."
        )
    }

    func hover(_ params: HoverParams) throws -> HoverResult {
        _ = try requirePid(params.pid, method: "input.hover")
        throw backgroundMouseUndeliverable(
            "A background hover",
            alternative: "Read the element under the point with `ax_query` / `som` instead of moving "
                + "a phantom pointer to provoke its hover state."
        )
    }

    func mouseButton(_ params: MouseButtonParams) throws -> MouseButtonResult {
        _ = try requirePid(params.pid, method: "input.mouse_button")
        throw backgroundMouseUndeliverable(
            "A background mouse press/release at a raw coordinate",
            alternative: "Press a control directly with `ax_action` (which presses and releases the "
                + "element), or hold a key across actions with `key_button`."
        )
    }

    /// The real cursor's position. Reported top-left origin, matching the screen
    /// point space every other coordinate on this rail uses (`NSEvent.mouseLocation`
    /// is bottom-left and would be off by the screen height).
    func cursorPosition() throws -> CursorPositionResult {
        guard let location = CGEvent(source: nil)?.location else {
            throw RpcError(code: -32_603, message: "CGEvent(source:) returned nil; cursor position unavailable", data: nil)
        }
        return CursorPositionResult(x: Double(location.x), y: Double(location.y))
    }

    // MARK: Click rails

    /// The AX-first click ladder.
    ///
    /// Rung 1 — ask AX what is at the point INSIDE the target app, and if that
    /// element (or, failing that, the smallest actionable element within
    /// `axLadderDepth` below it that still contains the point) advertises the
    /// action a click performs, perform the action. No event is synthesized at
    /// all: it is the same rail `ax.perform_action` already uses, so it works on
    /// an app that is not frontmost, occluded, or on another Space, and it hits
    /// the control rather than a coordinate that may have moved since the
    /// screenshot the model is looking at.
    ///
    /// Rung 2 — otherwise synthesize a targeted mouse event.
    ///
    /// A multi-click never takes rung 1: click COUNT is the payload of a
    /// double-click (an app opens on count 2), and `AXPress` carries no count, so
    /// pressing twice would deliver two singles — precisely the bug this fixes.
    private func performClick(pid: pid_t, x: Double, y: Double, button: MouseButton, count: Int) throws -> ClickResult {
        if count == 1, let matched = try axClick(pid: pid, x: x, y: y, button: button) {
            return ClickResult(ok: true, delivery: DELIVERY_TARGETED, path: "accessibility", matched: matched)
        }
        // No AX rung took the click, so the only thing left is a synthesized mouse
        // event — and on macOS that cannot run in the background (see
        // `backgroundMouseUndeliverable`). A multi-click never has an AX rung at
        // all (AXPress carries no count), so it lands here by definition.
        throw backgroundMouseUndeliverable(
            count > 1 ? "A background double-click" : "A background click at a raw coordinate",
            alternative: "Target a control instead — `ax_action` (or `set_value` for a field) presses "
                + "the element directly and in the background, and hits the control rather than a "
                + "coordinate that may have moved since the screenshot."
        )
    }

    /// Rung 1. Returns the element acted on, or `nil` when no rung-1 candidate
    /// exists (which is not an error — it means "synthesize instead").
    private func axClick(pid: pid_t, x: Double, y: Double, button: MouseButton) throws -> AxElement? {
        let wanted = axActionsStandingInForClick(button)
        guard !wanted.isEmpty, AXIsProcessTrusted() else { return nil }

        // Hit-test against the APPLICATION element, never `AXUIElementCreateSystemWide()`.
        // A system-wide hit test resolves whatever window owns the pixel, which may
        // belong to a different process than the one the caller named and the
        // approval gate judged — clicking a background 1Password sheet while the
        // gate reasoned about Chrome. Scoping the hit test to `pid` makes that
        // impossible by construction.
        let app = AXUIElementCreateApplication(pid)
        var hit: AXUIElement?
        guard AXUIElementCopyElementAtPosition(app, Float(x), Float(y), &hit) == .success,
              let hitElement = hit
        else { return nil }

        // Collect the hit element and its descendants, keeping only those that
        // contain the point AND advertise a stand-in action.
        var candidates: [(element: AXUIElement, action: String, meta: AxElement)] = []
        var ranked: [(area: Double, depth: Int, index: Int)] = []
        var visited = 0

        func consider(_ element: AXUIElement, depth: Int) {
            guard visited < Self.axLadderMaxNodes else { return }
            visited += 1

            if let (action, meta) = actionableMeta(element, wanted: wanted, pid: pid, x: x, y: y),
               let bounds = meta.bounds {
                ranked.append((area: bounds.width * bounds.height, depth: depth, index: candidates.count))
                candidates.append((element: element, action: action, meta: meta))
            }
            guard depth < Self.axLadderDepth else { return }
            for child in (axAttr(element, kAXChildrenAttribute) as? [AXUIElement] ?? []) {
                consider(child, depth: depth + 1)
            }
        }
        consider(hitElement, depth: 0)

        guard let winner = rankClickCandidates(ranked).map({ candidates[$0] }) else { return nil }
        let err = AXUIElementPerformAction(winner.element, winner.action as CFString)
        // A refused AX action is not a failure to report — it is simply not the
        // rung that runs. Falling through to a synthesized event is the ladder
        // working as designed, and the result says `path: "synthetic"` so the
        // model is never told an element was pressed when it was not.
        guard err == .success else { return nil }
        return winner.meta
    }

    /// The action this element offers for the click, plus its metadata — or `nil`
    /// when it offers none, is not inside the point, or is not ours.
    private func actionableMeta(
        _ element: AXUIElement,
        wanted: [String],
        pid: pid_t,
        x: Double,
        y: Double
    ) -> (String, AxElement)? {
        // AX children can cross process boundaries (a hosted view). The approval
        // gate authorised an act on `pid`; anything else is out of scope.
        var owner: pid_t = 0
        guard AXUIElementGetPid(element, &owner) == .success, owner == pid else { return nil }

        guard let bounds = boundsOf(element), boundsContain(bounds, x: x, y: y) else { return nil }

        var names: CFArray?
        guard AXUIElementCopyActionNames(element, &names) == .success,
              let actions = names as? [String],
              let action = wanted.first(where: { actions.contains($0) })
        else { return nil }

        // A disabled control still advertises AXPress and still refuses it.
        // Skipping it here lets the ladder keep looking (at the enabled control
        // underneath, or at the synthetic rung) instead of spending its one AX
        // shot on an element that was never going to act.
        let enabled = axAttr(element, kAXEnabledAttribute) as? Bool
        if enabled == false { return nil }

        let role = (axAttr(element, kAXRoleAttribute) as? String) ?? "AXUnknown"
        let meta = AxElement(
            role: role,
            title: axAttr(element, kAXTitleAttribute) as? String,
            value: nil,
            bounds: bounds,
            pid: owner,
            secure: isSecureSubrole(axAttr(element, kAXSubroleAttribute) as? String),
            enabled: enabled,
            settable: nil,
            actions: actions,
            url: nil,
            children: []
        )
        return (action, meta)
    }

    // MARK: Event plumbing

    /// Error code for "this pointer act cannot be delivered to a background
    /// process on macOS". Outside the JSON-RPC reserved band, and distinct from
    /// -32602 (bad params) / -32603 (internal): the request was well-formed and
    /// nothing failed internally — the platform simply offers no background rail
    /// for it, and the caller must choose another approach.
    private static let backgroundMouseUndeliverableCode: Int32 = -32_050

    /// The honest refusal for a synthesized mouse act with no AX rung.
    ///
    /// A located mouse event handed to `CGEvent.postToPid` reaches the target
    /// process but is never associated with a window by the window server, so the
    /// app does not act on it — verified on macOS 27: the identical event posted
    /// to the global HID tap DOES act, `postToPid` does not. The only background,
    /// cursor-free coordinate actuation macOS offers is the AX rail (pressing an
    /// element that advertises the action). When that is unavailable, actuating
    /// the coordinate at all requires the global tap, which drags the user's real
    /// cursor — no longer "background". Silently posting to the global tap (an
    /// invisible decision to disturb the user, made in the limb) or returning a
    /// success the app never saw (the exact lie the e2e caught) are both refused.
    /// The limb states the fact and hands the choice up; whether to fall back to
    /// the visible cursor rail is the caller's policy (`allow_global_pointer`),
    /// not the bridge's to make.
    private func backgroundMouseUndeliverable(_ what: String, alternative: String) -> RpcError {
        RpcError(
            code: Self.backgroundMouseUndeliverableCode,
            message: "\(what) cannot be delivered to a background process on macOS: a synthesized "
                + "mouse event posted to a pid is not routed to a window, so the target never acts "
                + "on it. \(alternative) To use the visible cursor instead — which physically moves "
                + "and requires the app frontmost — the operator can enable the global pointer rail.",
            data: nil
        )
    }

    private func eventSource() throws -> CGEventSource {
        guard let source else {
            throw RpcError(
                code: -32_603,
                message: "CGEventSource(stateID: .combinedSessionState) returned nil; cannot synthesize input",
                data: nil
            )
        }
        return source
    }

    /// `pid` is required. A missing one is NOT quietly turned into a global-tap
    /// click here: that would drag the user's real cursor away from whatever they
    /// are doing, and it would do it as an invisible decision made in the limb.
    /// Whether to fall back is routing policy, it lives in Rust, and the model has
    /// to be told which rail ran (`delivery`).
    private func requirePid(_ pid: Int32?, method: String) throws -> pid_t {
        guard let pid else {
            throw RpcError(
                code: -32_602,
                message: "\(method) requires `pid`: the targeted rail posts into one process's event queue. "
                    + "Falling back to the global HID tap is a routing decision for the caller, not the bridge.",
                data: nil
            )
        }
        return pid_t(pid)
    }

    private func postKey(
        source: CGEventSource,
        code: CGKeyCode,
        down: Bool,
        flags: CGEventFlags,
        pid: pid_t
    ) throws {
        guard let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) else {
            throw RpcError(code: -32_603, message: "CGEvent(keyboardEventSource:) returned nil", data: nil)
        }
        // Set exactly the requested chord. Unlike the mouse rail, inheriting the
        // session's flags here would corrupt the request: a ⇧ the user happens to
        // be holding would silently turn ⌘Z into ⌘⇧Z (redo, not undo).
        event.flags = flags
        event.postToPid(pid)
    }

    private func axAttr(_ element: AXUIElement, _ name: String) -> AnyObject? {
        var value: AnyObject?
        return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
    }

    private func boundsOf(_ element: AXUIElement) -> Region? {
        var posVal: AnyObject?
        var sizeVal: AnyObject?
        guard AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &posVal) == .success,
              AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &sizeVal) == .success,
              let pv = posVal, let sv = sizeVal
        else { return nil }
        var point = CGPoint.zero
        var size = CGSize.zero
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
