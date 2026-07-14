import AppKit
import Foundation

// AlephFixture — a tiny AppKit app that tells the truth about itself.
//
// It is the far end of the computer-use e2e loop: `desktop/macos/tests/
// computer_use_e2e.rs` drives the REAL bridge (real AX calls, real CGEvents) at
// this app, and then asks this app what actually happened. Two independent
// witnesses. Ask the bridge to verify its own writes and a bridge that does
// nothing but echo passes every test it has.
//
// It is never shipped: only `just swift-fixture` builds it and only the
// `#[ignore]`d e2e suite runs it. Nothing in AlephBridge knows it exists — there
// is deliberately no "fixture mode" branch inside the production actuation path,
// because a test-mode branch means the code under test is not the code that
// ships.
//
// Ground truth goes to the file named by ALEPH_FIXTURE_STATE, atomically, on
// every change. See GroundTruth.swift for the invariant that makes `seq` a
// trustworthy signal.
//
// Environment:
//   ALEPH_FIXTURE_STATE     (required) path the state JSON is written to
//   ALEPH_FIXTURE_MODE      headless (default) | visible
//   ALEPH_FIXTURE_TTL_SECS  self-destruct timer, default 120

/// The secure field's contents. `computer_use_e2e.rs` hardcodes the same literal
/// and asserts it appears NOWHERE in an AX snapshot: the test needs to know the
/// secret in order to prove the bridge never carried it, so the two sides share
/// it as a constant rather than the fixture publishing it (publishing the secret
/// in the ground-truth file would be the very leak under test).
let secureSentinel = "aleph-fixture-secret-9F3A21"

/// Marker written in place of the secure field's value in the state file.
let secureRedaction = "<redacted>"

/// One control the fixture owns, plus how to read its value.
struct Control {
    let identifier: String
    let role: String
    let title: String?
    let view: NSView
    let secure: Bool
    /// What the fixture DECLARES it offers, not what AppKit's AX happens to
    /// advertise on top (see `Element.actions`).
    let actions: [String]
    let value: () -> String?
}

final class FixtureDelegate: NSObject, NSApplicationDelegate, NSTextFieldDelegate {
    private let mode: String
    private let statePath: URL
    private let ttl: TimeInterval

    private var window: FixtureWindow!
    private var controls: [Control] = []
    private var truth: GroundTruth!
    private var counter = 0
    private var timers: [Timer] = []

    private var textField: NSTextField!
    private var scrollView: NSScrollView!

    init(mode: String, statePath: URL, ttl: TimeInterval) {
        self.mode = mode
        self.statePath = statePath
        self.ttl = ttl
        super.init()
    }

    // MARK: Launch

    func applicationDidFinishLaunching(_ notification: Notification) {
        buildWindow()

        truth = GroundTruth(path: statePath, mode: mode) { [unowned self] in self.observe() }
        // The first write publishes pid + geometry, which the Rust side needs
        // before it can act on anything at all.
        truth.record(kind: "ready")

        // The poll is what catches a mutation AppKit never told us about — an
        // `ax.set_value` write fires no delegate callback. See `Snapshot`.
        let poll = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
            self?.truth.poll()
        }
        // A panicking test must not leave a window sitting on the user's screen
        // forever. The Rust side kills the child on drop; this is the backstop for
        // when the Rust side is what died.
        let reaper = Timer.scheduledTimer(withTimeInterval: ttl, repeats: false) { _ in
            NSApp.terminate(nil)
        }
        timers = [poll, reaper]
    }

    private func buildWindow() {
        let content = NSRect(x: 0, y: 0, width: 520, height: 420)
        window = FixtureWindow(
            contentRect: content,
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Aleph Fixture"

        let button = NSButton(frame: NSRect(x: 20, y: 350, width: 220, height: 32))
        // An NSButton's title IS its AXTitle, and the AX locator matches on
        // role + title (the bridge never puts an accessibilityIdentifier on the
        // wire — see AxElement). So every control below is given a UNIQUE,
        // unmistakable title; that is what makes it addressable at all.
        button.title = "Aleph Counter Button"
        button.bezelStyle = .rounded
        button.target = self
        button.action = #selector(buttonPressed)
        button.setAccessibilityIdentifier("aleph.button")
        window.contentView?.addSubview(button)

        textField = NSTextField(frame: NSRect(x: 20, y: 300, width: 300, height: 24))
        textField.stringValue = ""
        textField.delegate = self
        textField.setAccessibilityIdentifier("aleph.textfield")
        // A plain NSTextField has no AXTitle of its own; without this it would be
        // indistinguishable from the secure field below, which shares its AX role.
        textField.setAccessibilityTitle("Aleph Text Field")
        window.contentView?.addSubview(textField)

        let secureField = NSSecureTextField(frame: NSRect(x: 20, y: 260, width: 300, height: 24))
        secureField.stringValue = secureSentinel
        secureField.setAccessibilityIdentifier("aleph.secure")
        secureField.setAccessibilityTitle("Aleph Secure Field")
        window.contentView?.addSubview(secureField)

        scrollView = NSScrollView(frame: NSRect(x: 20, y: 60, width: 300, height: 180))
        let document = FlippedView(frame: NSRect(x: 0, y: 0, width: 300, height: 4000))
        scrollView.documentView = document
        scrollView.hasVerticalScroller = true
        scrollView.setAccessibilityIdentifier("aleph.scroll")
        scrollView.setAccessibilityTitle("Aleph Scroll Area")
        scrollView.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(scrolled),
            name: NSView.boundsDidChangeNotification,
            object: scrollView.contentView
        )
        window.contentView?.addSubview(scrollView)

        let pad = PadView(frame: NSRect(x: 340, y: 60, width: 160, height: 320))
        pad.setAccessibilityIdentifier("aleph.dragpad")
        pad.onEvent = { [weak self] kind, value in
            self?.truth.record(kind: kind, element: "aleph.dragpad", value: value)
        }
        window.contentView?.addSubview(pad)

        controls = [
            Control(
                identifier: "aleph.button", role: "AXButton", title: "Aleph Counter Button",
                view: button, secure: false, actions: ["AXPress"], value: { nil }
            ),
            Control(
                identifier: "aleph.textfield", role: "AXTextField", title: "Aleph Text Field",
                view: textField, secure: false, actions: [],
                value: { [unowned self] in self.textField.stringValue }
            ),
            Control(
                identifier: "aleph.secure", role: "AXTextField", title: "Aleph Secure Field",
                view: secureField, secure: true, actions: [],
                // The fixture does not publish the secret either: the Rust side
                // already knows it (`secureSentinel`), and writing it here would
                // put it in a file the test greps for leaks.
                value: { secureRedaction }
            ),
            Control(
                identifier: "aleph.scroll", role: "AXScrollArea", title: "Aleph Scroll Area",
                view: scrollView, secure: false, actions: [],
                // Whole points: sub-pixel jitter here would churn `seq` on every
                // poll tick and destroy its meaning as a change signal.
                value: { [unowned self] in String(format: "%.0f", self.scrollView.contentView.bounds.origin.y) }
            ),
            Control(
                identifier: "aleph.dragpad", role: "AXGroup", title: nil,
                view: pad, secure: false, actions: [], value: { nil }
            ),
        ]

        place()
    }

    /// Where the window goes, and whether the app is allowed to come forward.
    ///
    /// **headless** — the window is ordered in but parked off every display, and
    /// the app is an `.accessory` (no Dock icon, never activated). The AX tree is
    /// live, nothing is on screen, and the user's focus is untouched. It is NOT
    /// `orderOut`: a hidden window leaves `NSApp.windows`' visible set and with it
    /// the app's AX children, so `ax.query_tree` would come back empty and every
    /// Tier A assertion would fail against a fixture that was working fine.
    ///
    /// Parking it off-display buys the same thing (invisible, never frontmost)
    /// while keeping the window — and therefore the AX tree — real. It also keeps
    /// the Tier A / Tier B split honest: no real screen coordinate intersects this
    /// window, so a coordinate click cannot accidentally land and hand a headless
    /// run a green it did not earn.
    ///
    /// **visible** — a normal, activated, on-screen app. Only here can posted
    /// CGEvents land, which is why Tier B insists on it.
    private func place() {
        switch mode {
        case "visible":
            NSApp.setActivationPolicy(.regular)
            window.setFrameOrigin(NSPoint(x: 120, y: 120))
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            // type_text posts to the app's KEY WINDOW focus, so the test needs a
            // known first responder. Establishing it here — rather than clicking
            // the field — is deliberate: a left click on an NSTextField is
            // satisfied by the bridge's AX rail (`AXConfirm` is a click stand-in),
            // which does not move focus.
            window.makeFirstResponder(textField)
        default:
            NSApp.setActivationPolicy(.accessory)
            window.setFrameOrigin(NSPoint(x: -30000, y: -30000))
            // Ordered in (so the AX tree exists) without activating the app.
            window.orderFrontRegardless()
        }
    }

    // MARK: Events

    @objc private func buttonPressed() {
        counter += 1
        truth.record(kind: "button_press", element: "aleph.button", value: "counter=\(counter)")
    }

    @objc private func scrolled() {
        let offset = String(format: "%.0f", scrollView.contentView.bounds.origin.y)
        truth.record(kind: "scroll", element: "aleph.scroll", value: "offset=\(offset)")
    }

    func controlTextDidChange(_ notification: Notification) {
        truth.record(kind: "text", element: "aleph.textfield", value: textField.stringValue)
    }

    // MARK: Observation

    private func observe() -> Snapshot {
        Snapshot(
            window_bounds: cgRect(fromScreenRect: window.frame),
            focused: focusedIdentifier(),
            counter: counter,
            elements: controls.map {
                Element(
                    identifier: $0.identifier,
                    role: $0.role,
                    title: $0.title,
                    value: $0.value(),
                    secure: $0.secure,
                    actions: $0.actions,
                    frame: cgFrame(of: $0.view)
                )
            }
        )
    }

    private func focusedIdentifier() -> String? {
        guard let responder = window?.firstResponder else { return nil }
        var focusedView: NSView?
        // While a text field is being edited the first responder is the window's
        // shared FIELD EDITOR, not the field itself. Resolve it back to the
        // control it is editing, or focus would report as "some NSTextView" and
        // the type_text precondition could never be checked.
        if let editor = responder as? NSText, let owner = editor.delegate as? NSView {
            focusedView = owner
        } else if let view = responder as? NSView {
            focusedView = view
        }
        guard let focusedView else { return nil }
        return controls.first { $0.view === focusedView }?.identifier
    }
}

// MARK: - Entry point

guard let rawPath = ProcessInfo.processInfo.environment["ALEPH_FIXTURE_STATE"], !rawPath.isEmpty else {
    FileHandle.standardError.write(Data("aleph-fixture: ALEPH_FIXTURE_STATE is required\n".utf8))
    exit(2)
}
let environment = ProcessInfo.processInfo.environment
let fixtureMode = environment["ALEPH_FIXTURE_MODE"] ?? "headless"
let fixtureTtl = TimeInterval(environment["ALEPH_FIXTURE_TTL_SECS"] ?? "") ?? 120

let app = NSApplication.shared
let delegate = FixtureDelegate(
    mode: fixtureMode,
    statePath: URL(fileURLWithPath: rawPath),
    ttl: fixtureTtl
)
app.delegate = delegate
app.run()
