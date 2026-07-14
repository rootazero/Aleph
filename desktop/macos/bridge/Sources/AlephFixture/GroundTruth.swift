import AppKit
import Foundation

// MARK: - Wire format
//
// This file is the entire reason the fixture exists.
//
// The e2e suite drives the BRIDGE and then asks the APP what happened. Those are
// two independent witnesses. The alternative — act through the bridge, then read
// back through the bridge — verifies one rail against itself: if `ax.set_value`
// writes nowhere and `ax.query_tree` reports the value it was told to write, the
// round trip is green and the app never changed. A fixture that reports its own
// state is what makes that lie impossible to tell.
//
// Every field below is therefore something only the app can testify to.

/// A rectangle in the bridge's coordinate space: global screen POINTS, top-left
/// origin.
///
/// AppKit is bottom-left-origin; accessibility (`kAXPosition`) and CoreGraphics
/// events are top-left. The fixture publishes the bridge's space, never AppKit's,
/// because the Rust side feeds these frames straight back into `input.click` —
/// converting once here beats converting at every call site, and a mismatch
/// between the two spaces is a bug class this fixture is meant to CATCH (see
/// `cgRect(fromScreenRect:)`).
struct Rect: Codable, Equatable {
    var x: Double
    var y: Double
    var width: Double
    var height: Double
}

/// One control, as the app itself sees it.
///
/// `actions` is what the fixture *declares* it offers (a button offers `AXPress`),
/// not what AX happens to advertise — AppKit adds its own (`AXShowMenu`, …). The
/// Rust side asserts the bridge's AX actions are a SUPERSET of this, which is the
/// true relation between the two.
struct Element: Codable, Equatable {
    var identifier: String
    var role: String
    var title: String?
    /// `nil` when the control has no value. For the secure field this is the
    /// redaction marker — never the secret (see `secureSentinel`).
    var value: String?
    var secure: Bool
    var actions: [String]
    var frame: Rect
}

/// The last thing that happened TO the app, as the app saw it.
///
/// `value` carries the detail that only the receiving app knows: the click count
/// carried on a mouse event, the number of intermediate drag steps. Those are the
/// facts the Wave-1/Wave-3 fixes are actually about, and they are invisible from
/// the sending side — a rail that posts two independent single clicks and a rail
/// that posts one real double-click both "succeed" as far as the sender can tell.
struct LastEvent: Codable, Equatable {
    var kind: String
    var element: String?
    var value: String?
    var seq: UInt64
}

/// The part of the state that can change without AppKit telling the fixture.
///
/// `ax.set_value` writes into the control through the accessibility API, and
/// AppKit fires NO delegate callback for it (`controlTextDidChange` is a
/// field-editor signal, not a `setStringValue:` signal). A purely event-driven
/// fixture would therefore never notice an AX write, `seq` would never advance,
/// and the Rust test would hang waiting for it. The poll diffs this struct
/// instead — so the fixture notices a change no matter which rail caused it.
struct Snapshot: Codable, Equatable {
    var window_bounds: Rect
    var focused: String?
    var counter: Int
    var elements: [Element]
}

/// What lands on disk.
struct State: Codable {
    /// Every `input.*` call needs it: the targeted rail posts into ONE process's
    /// event queue, so the test cannot act at all without the fixture's pid.
    var pid: Int32
    var seq: UInt64
    var mode: String
    var window_bounds: Rect
    var focused: String?
    var counter: Int
    var elements: [Element]
    var last_event: LastEvent?
}

// MARK: - Writer

/// Owns `seq` and publishes `State` atomically.
///
/// **`seq` may never tick on its own.** The Rust side waits for `seq` to ADVANCE
/// instead of sleeping for a guessed duration; a `seq` that climbed by itself
/// (say, once per poll tick) would turn every one of those waits into a sleep
/// that always succeeds, and the suite would go green without the bridge doing
/// anything at all. That is precisely the fake green this fixture exists to kill,
/// so it is stated here as an invariant:
///
///   `seq` advances on a recorded event, or on a poll that observed a real
///   change. Never otherwise.
///
/// Main-thread only — every read below is an AppKit read. Not an actor on
/// purpose: AppKit callbacks and the poll `Timer` already run on the main run
/// loop, so there is nothing to serialise and an actor would only add hops.
final class GroundTruth {
    private let path: URL
    private let mode: String
    private let capture: () -> Snapshot

    private var seq: UInt64 = 0
    private var lastSnapshot: Snapshot?
    private var lastEvent: LastEvent?

    init(path: URL, mode: String, capture: @escaping () -> Snapshot) {
        self.path = path
        self.mode = mode
        self.capture = capture
    }

    /// An event is always news, so this always writes: two identical clicks are
    /// two facts, and the second one must still move `seq` or a test waiting on
    /// it would hang forever.
    func record(kind: String, element: String? = nil, value: String? = nil) {
        let snapshot = capture()
        seq += 1
        lastEvent = LastEvent(kind: kind, element: element, value: value, seq: seq)
        lastSnapshot = snapshot
        write(snapshot)
    }

    /// Poll tick: write only when something actually moved.
    func poll() {
        let snapshot = capture()
        guard snapshot != lastSnapshot else { return }
        seq += 1
        lastSnapshot = snapshot
        write(snapshot)
    }

    private func write(_ snapshot: Snapshot) {
        let state = State(
            pid: ProcessInfo.processInfo.processIdentifier,
            seq: seq,
            mode: mode,
            window_bounds: snapshot.window_bounds,
            focused: snapshot.focused,
            counter: snapshot.counter,
            elements: snapshot.elements,
            last_event: lastEvent
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        do {
            let data = try encoder.encode(state)
            // `.atomic` writes a sibling temp file and rename(2)s it into place, so
            // a reader polling this path sees either the whole old state or the
            // whole new one — never a half-written file. The Rust side reads it
            // while we are rewriting it, so this is load-bearing, not hygiene.
            try data.write(to: path, options: [.atomic])
        } catch {
            let msg = "aleph-fixture: state write to \(path.path) failed: \(error)\n"
            FileHandle.standardError.write(Data(msg.utf8))
        }
    }
}

// MARK: - Coordinate conversion

/// AppKit screen rect (bottom-left origin) → bridge rect (top-left origin).
///
/// The origin of AppKit's global space is the PRIMARY display's bottom-left, so
/// the flip is against that display's height — `NSScreen.screens.first`, never
/// `NSScreen.main` (which is whichever screen holds the key window).
func cgRect(fromScreenRect rect: NSRect) -> Rect {
    let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
    return Rect(
        x: Double(rect.origin.x),
        y: Double(primaryHeight - rect.origin.y - rect.height),
        width: Double(rect.width),
        height: Double(rect.height)
    )
}

/// A view's frame in the bridge's coordinate space.
func cgFrame(of view: NSView) -> Rect {
    guard let window = view.window else { return Rect(x: 0, y: 0, width: 0, height: 0) }
    let inWindow = view.convert(view.bounds, to: nil)
    return cgRect(fromScreenRect: window.convertToScreen(inWindow))
}
