//! Closed-loop computer-use e2e: Rust `SwiftBridge` → real `AlephBridge` → real
//! macOS (AX + CGEvent) → a real app, `AlephFixture`, which reports back what
//! actually happened to it.
//!
//! Run with `just test-computer-use-e2e`. Everything here is `#[ignore]`: it needs
//! Accessibility (TCC) and, for Tier B, a real logged-in GUI session.
//!
//! # Why a fixture, and why it reports its own state
//!
//! Every assertion below is checked against `AlephFixture`'s own account of
//! itself, never against the bridge's read-back of its own write. A bridge that
//! writes nowhere and echoes the value it was handed passes a round-trip test
//! perfectly — self-verification proves nothing. The fixture is an INDEPENDENT
//! witness, and the facts it testifies to (the click COUNT carried on an event,
//! whether a drag walked a path, which control holds focus) are facts the sending
//! side physically cannot see.
//!
//! The fixture is a separate process, not a test hook inside the bridge. There is
//! deliberately no fixture-mode branch in the production actuation path: a branch
//! like that forks test-mode into every entry point and means the path under test
//! is not the path that ships. Because this suite is opt-in, it can afford the
//! strictly stronger thing — driving REAL CGEvents at a REAL app.
//!
//! # The two tiers, and why folding them together would be a lie
//!
//! **Tier A — AX rail, headless.** The fixture's window is parked off every
//! display and its app is an `.accessory` (never frontmost, nothing on screen,
//! the user's focus untouched). The AX tree is still live, so `ax.perform_action`
//! and `ax.set_value` work. No coordinates are used.
//!
//! **Tier B — CGEvent rail, visible.** Posted mouse/keyboard events only land on a
//! window that is actually on screen in an active GUI session. Tier B therefore
//! runs the fixture visible and drives it by coordinate.
//!
//! These must never be merged. A "click" test run against an off-screen window
//! posts an event that lands nowhere, and every assertion about it would have to
//! be weakened to nothing to keep it green — a vacuous pass, which is exactly the
//! disease this file was written to cure.

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aleph_desktop::bridge::client::SwiftBridge;
use aleph_protocol::desktop_bridge::methods::ax::{
    AxActionResult, AxElement, AxLocator, PerformActionParams, QueryByRoleParams, QueryListResult,
    QueryResult, QueryTreeParams, SetValueParams,
};
use aleph_protocol::desktop_bridge::methods::input::{
    ClickParams, CursorPositionResult, DragParams, MouseButton, ScrollParams, TypeTextParams,
    DELIVERY_TARGETED,
};
use serde::Deserialize;

// ── Contract shared with the fixture ─────────────────────────────────────────

/// Must match `secureSentinel` in `Sources/AlephFixture/main.swift`.
///
/// The test has to KNOW the secret in order to prove the bridge never carried it,
/// so it is a shared constant rather than something the fixture publishes — the
/// fixture writing its own password into the ground-truth file would be the very
/// leak under test.
const SECURE_SENTINEL: &str = "aleph-fixture-secret-9F3A21";

const BUTTON_TITLE: &str = "Aleph Counter Button";
const TEXT_FIELD_TITLE: &str = "Aleph Text Field";
const SECURE_FIELD_TITLE: &str = "Aleph Secure Field";

/// CJK + emoji + a combining accent. Typed and set verbatim, asserted
/// byte-identically: this is the payload that a UTF-16 chunker splitting a
/// grapheme cluster would mangle.
const UNICODE_PAYLOAD: &str = "你好世界 🌍 café";

/// How long any single observation may take to show up in the fixture's state.
const OBSERVE_TIMEOUT: Duration = Duration::from_secs(5);

// ── Fixture state (the wire format written by GroundTruth.swift) ─────────────

#[derive(Debug, Clone, Copy, Deserialize)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct FixtureElement {
    identifier: String,
    #[allow(dead_code)]
    role: String,
    #[allow(dead_code)]
    title: Option<String>,
    value: Option<String>,
    #[allow(dead_code)]
    secure: bool,
    /// What the fixture declares it offers. The bridge's AX `actions` must be a
    /// superset of this.
    actions: Vec<String>,
    frame: Rect,
}

#[derive(Debug, Clone, Deserialize)]
struct LastEvent {
    kind: String,
    element: Option<String>,
    value: Option<String>,
    #[allow(dead_code)]
    seq: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct FixtureState {
    pid: i32,
    seq: u64,
    focused: Option<String>,
    counter: i64,
    elements: Vec<FixtureElement>,
    last_event: Option<LastEvent>,
}

impl FixtureState {
    fn element(&self, identifier: &str) -> &FixtureElement {
        self.elements
            .iter()
            .find(|e| e.identifier == identifier)
            .unwrap_or_else(|| panic!("fixture reports no element {identifier}: {self:?}"))
    }

    fn last_event(&self) -> &LastEvent {
        self.last_event
            .as_ref()
            .unwrap_or_else(|| panic!("fixture reports no last_event: {self:?}"))
    }
}

// ── Harness ─────────────────────────────────────────────────────────────────

fn helper_path() -> PathBuf {
    // CARGO_MANIFEST_DIR for this crate is `desktop/macos`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("bridge")
        .join(".build")
        .join("release")
        .join("AlephBridge")
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("bridge")
        .join(".build")
        .join("release")
        .join("AlephFixture")
}

fn bridge() -> SwiftBridge {
    let path = helper_path();
    assert!(
        path.exists(),
        "helper not built at {}; run `just swift-bridge` first",
        path.display()
    );
    SwiftBridge::new(path)
}

#[derive(Clone, Copy)]
enum Mode {
    /// Off-display, `.accessory`, never frontmost. AX rail only.
    Headless,
    /// On screen and activated. Required for anything posted as a CGEvent.
    Visible,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Headless => "headless",
            Mode::Visible => "visible",
        }
    }
}

/// A running `AlephFixture`, killed on drop.
struct Fixture {
    child: Child,
    state_path: PathBuf,
}

impl Fixture {
    fn launch(mode: Mode) -> Self {
        let binary = fixture_path();
        assert!(
            binary.exists(),
            "fixture not built at {}; run `just swift-fixture` first",
            binary.display()
        );

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let state_path = std::env::temp_dir().join(format!(
            "aleph-fixture-{}-{unique}.json",
            std::process::id()
        ));

        let child = Command::new(&binary)
            .env("ALEPH_FIXTURE_STATE", &state_path)
            .env("ALEPH_FIXTURE_MODE", mode.as_str())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn fixture {}: {e}", binary.display()));

        let fixture = Fixture { child, state_path };
        // The fixture publishes pid + geometry on launch; nothing can be driven
        // before that lands.
        fixture.wait_until("fixture to publish its initial state", |_| true);
        fixture
    }

    fn try_state(&self) -> Option<FixtureState> {
        // `.atomic` on the writing side means rename(2), so a read here sees either
        // the whole previous state or the whole next one — never a torn file. A
        // parse failure is a real bug, not a race, and is surfaced as one.
        let raw = fs::read_to_string(&self.state_path).ok()?;
        match serde_json::from_str(&raw) {
            Ok(state) => Some(state),
            Err(e) => panic!("fixture wrote unparseable state ({e}): {raw}"),
        }
    }

    fn state(&self) -> FixtureState {
        self.try_state()
            .unwrap_or_else(|| panic!("fixture has not written {} yet", self.state_path.display()))
    }

    /// Poll the fixture's ground truth until `predicate` holds.
    ///
    /// This is what replaces sleeping: the fixture bumps `seq` only when something
    /// really changed (see GroundTruth.swift), so waiting on an observed fact is
    /// both faster than a guessed delay and — unlike a delay — actually capable of
    /// failing when the bridge did nothing.
    ///
    /// The blocking sleep is safe on the test runtime: every bridge call is awaited
    /// to completion before any wait begins, so there is never an in-flight RPC for
    /// this to starve.
    fn wait_until(&self, what: &str, predicate: impl Fn(&FixtureState) -> bool) -> FixtureState {
        let deadline = Instant::now() + OBSERVE_TIMEOUT;
        let mut last = None;
        while Instant::now() < deadline {
            if let Some(state) = self.try_state() {
                if predicate(&state) {
                    return state;
                }
                last = Some(state);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "timed out after {:?} waiting for {what}; last state: {last:#?}",
            OBSERVE_TIMEOUT
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.state_path);
    }
}

fn locator(pid: i32, role: &str, title: &str) -> AxLocator {
    AxLocator {
        pid: Some(pid),
        role: Some(role.to_string()),
        title: Some(title.to_string()),
        center: None,
    }
}

// ── Tier A — AX rail, headless ───────────────────────────────────────────────

/// The AX tree of an app with nothing on screen is live, and pressing through it
/// really presses.
///
/// Also pins the affordance contract from Wave 2: the actions the bridge reports
/// must be a SUPERSET of what the fixture declares it offers. (AppKit adds its
/// own; the fixture only claims `AXPress`.)
#[tokio::test]
#[ignore]
async fn tier_a_ax_press_increments_the_fixtures_own_counter() {
    let fixture = Fixture::launch(Mode::Headless);
    let bridge = bridge();
    let before = fixture.state();

    let found: QueryListResult = bridge
        .call(
            "ax.query_by_role",
            QueryByRoleParams {
                role: "AXButton".to_string(),
                pid: Some(before.pid),
            },
        )
        .await
        .expect("ax.query_by_role failed");

    let button = found
        .elements
        .iter()
        .find(|e| e.title.as_deref() == Some(BUTTON_TITLE))
        .unwrap_or_else(|| {
            panic!(
                "the bridge cannot see the fixture's button through AX while the window is \
                 off-display. Either accessibility is not granted to the helper, or the fixture's \
                 window is no longer part of its AX tree. Elements seen: {:?}",
                found.elements
            )
        });

    let advertised = button
        .actions
        .as_ref()
        .expect("helper omitted `actions` — the Wave 2 affordances are missing");
    // The relation is superset, not equality: the fixture declares what it offers
    // (`AXPress`), and AppKit adds its own on top (`AXShowMenu`, …). Deriving the
    // assertion from the fixture's declaration rather than hardcoding it here keeps
    // the two sides honest about each other.
    let declared = &before.element("aleph.button").actions;
    assert!(
        !declared.is_empty(),
        "the fixture declares no actions for its button"
    );
    for action in declared {
        assert!(
            advertised.contains(action),
            "the fixture offers {action} but the bridge does not advertise it; got {advertised:?}"
        );
    }

    let result: AxActionResult = bridge
        .call(
            "ax.perform_action",
            PerformActionParams {
                locator: locator(before.pid, "AXButton", BUTTON_TITLE),
                action: "AXPress".to_string(),
            },
        )
        .await
        .expect("ax.perform_action(AXPress) failed");
    assert!(result.performed);
    assert_eq!(result.path, "accessibility");

    // The bridge said it pressed. Ask the app.
    let after = fixture.wait_until("the fixture's counter to increment", |s| {
        s.counter == before.counter + 1
    });
    assert_eq!(after.last_event().kind, "button_press");
    assert_eq!(after.last_event().element.as_deref(), Some("aleph.button"));
}

/// `ax.set_value` really writes, and CJK + emoji survive byte-identically.
#[tokio::test]
#[ignore]
async fn tier_a_ax_set_value_roundtrips_cjk_and_emoji_byte_identically() {
    let fixture = Fixture::launch(Mode::Headless);
    let bridge = bridge();
    let before = fixture.state();
    assert_eq!(
        before.element("aleph.textfield").value.as_deref(),
        Some(""),
        "fixture should start with an empty text field"
    );

    let result: AxActionResult = bridge
        .call(
            "ax.set_value",
            SetValueParams {
                locator: locator(before.pid, "AXTextField", TEXT_FIELD_TITLE),
                value: UNICODE_PAYLOAD.to_string(),
            },
        )
        .await
        .expect("ax.set_value failed");
    assert!(result.performed);

    let verification = result
        .verification
        .as_ref()
        .expect("set_value returned no verification");
    assert_eq!(
        verification.state, "verified",
        "bridge could not verify its own write: {verification:?}"
    );

    // And now the part the bridge cannot fake: the app's own view of its field.
    let after = fixture.wait_until("the fixture's text field to carry the payload", |s| {
        s.element("aleph.textfield").value.as_deref() == Some(UNICODE_PAYLOAD)
    });
    assert_eq!(
        after.element("aleph.textfield").value.as_deref(),
        Some(UNICODE_PAYLOAD)
    );
}

/// A secure field's contents never cross the bridge.
///
/// The fixture holds a known password. The assertion is not "the value looks
/// redacted" but "the sentinel appears NOWHERE in the raw AX snapshot" — the only
/// form of the claim that a partial leak (in a title, a preview, a nested node)
/// cannot slip past.
#[tokio::test]
#[ignore]
async fn tier_a_secure_field_value_never_appears_in_a_snapshot() {
    let fixture = Fixture::launch(Mode::Headless);
    let bridge = bridge();
    let state = fixture.state();

    let snapshot: serde_json::Value = bridge
        .call(
            "ax.query_tree",
            QueryTreeParams {
                pid: Some(state.pid),
                max_depth: 10,
            },
        )
        .await
        .expect("ax.query_tree failed");

    let raw = serde_json::to_string(&snapshot).expect("snapshot is not serialisable");
    assert!(
        !raw.contains(SECURE_SENTINEL),
        "the secure field's cleartext crossed the bridge in an AX snapshot"
    );

    // The snapshot must still SAY the field is secure — withholding the value is
    // only safe if the model is told the field exists and is a password.
    let typed: QueryResult =
        serde_json::from_value(snapshot).expect("snapshot is not a QueryResult");
    let root = typed.element.expect("ax.query_tree returned no element");
    let secure = find_by_title(&root, SECURE_FIELD_TITLE)
        .unwrap_or_else(|| panic!("secure field not found in the AX tree: {root:?}"));
    assert_eq!(
        secure.secure,
        Some(true),
        "the secure field is not flagged `secure`"
    );
}

/// The fixture's own geometry and the bridge's AX bounds agree.
///
/// Tier B aims real clicks using the frames the FIXTURE publishes, so the two
/// coordinate spaces (AppKit's bottom-left vs AX/CGEvent's top-left) must line up.
/// If they ever drift — a Retina scale factor creeping in, a flip against the
/// wrong screen — Tier B would start missing its targets for a reason that has
/// nothing to do with the input rail. Pin it here, where the failure is legible.
#[tokio::test]
#[ignore]
async fn tier_a_fixture_geometry_agrees_with_the_bridges_ax_bounds() {
    let fixture = Fixture::launch(Mode::Headless);
    let bridge = bridge();
    let state = fixture.state();

    let found: QueryListResult = bridge
        .call(
            "ax.query_by_role",
            QueryByRoleParams {
                role: "AXButton".to_string(),
                pid: Some(state.pid),
            },
        )
        .await
        .expect("ax.query_by_role failed");
    let button = found
        .elements
        .iter()
        .find(|e| e.title.as_deref() == Some(BUTTON_TITLE))
        .expect("button not found");
    let ax = button.bounds.as_ref().expect("button has no AX bounds");
    let own = state.element("aleph.button").frame;

    for (what, ax_v, own_v) in [
        ("x", ax.x, own.x),
        ("y", ax.y, own.y),
        ("width", ax.width, own.width),
        ("height", ax.height, own.height),
    ] {
        assert!(
            (ax_v - own_v).abs() <= 1.0,
            "{what}: the bridge's AX bounds ({ax_v}) and the fixture's own frame ({own_v}) \
             disagree by more than a point — the two coordinate spaces have drifted"
        );
    }
}

// ── Tier B — CGEvent rail, visible window ────────────────────────────────────

/// The AX-first click ladder (Wave 3) really takes the AX rung: a click on a
/// control that advertises `AXPress` presses the control, and the app confirms it.
#[tokio::test]
#[ignore]
async fn tier_b_click_takes_the_ax_rung_and_presses_the_button() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let before = fixture.state();
    let (x, y) = before.element("aleph.button").frame.center();

    let result: serde_json::Value = bridge
        .call(
            "input.click",
            ClickParams {
                x,
                y,
                button: MouseButton::Left,
                pid: Some(before.pid),
                click_count: None,
            },
        )
        .await
        .expect("input.click failed");

    assert_eq!(result["ok"], serde_json::json!(true));
    assert_eq!(result["delivery"], serde_json::json!(DELIVERY_TARGETED));
    // Rung 1: the ladder found an element advertising a click stand-in action and
    // pressed it, rather than synthesizing a mouse event at a coordinate.
    assert_eq!(
        result["path"],
        serde_json::json!("accessibility"),
        "click did not take the AX rung: {result}"
    );
    assert_eq!(result["matched"]["title"], serde_json::json!(BUTTON_TITLE));

    let after = fixture.wait_until("the fixture's counter to increment", |s| {
        s.counter == before.counter + 1
    });
    assert_eq!(after.last_event().kind, "button_press");
}

/// `input.double_click` delivers ONE event carrying click count 2 — not two
/// singles.
///
/// The drag pad is a plain view: it advertises no click stand-in action, so the
/// ladder falls through to a real synthesized mouse event, which is the rail under
/// test. The app is the only witness that can tell the two apart — a rail that
/// posts two independent single clicks looks identical from the sending side and
/// arrives as two `clickCount == 1` events.
#[tokio::test]
#[ignore]
async fn tier_b_double_click_delivers_click_count_two_not_two_singles() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let before = fixture.state();
    let (x, y) = before.element("aleph.dragpad").frame.center();

    let result: serde_json::Value = bridge
        .call(
            "input.double_click",
            ClickParams {
                x,
                y,
                button: MouseButton::Left,
                pid: Some(before.pid),
                // Ignored by `input.double_click` on purpose — the method name is the
                // request. Passing a contradictory count here is the point: if it
                // were honoured, the assertion below would fail.
                click_count: Some(1),
            },
        )
        .await
        .expect("input.double_click failed");
    assert_eq!(result["ok"], serde_json::json!(true));
    // A multi-click never takes the AX rung: AXPress carries no count.
    assert_eq!(result["path"], serde_json::json!("synthetic"));

    let after = fixture.wait_until("the pad to report a double click", |s| {
        s.seq > before.seq
            && s.last_event
                .as_ref()
                .is_some_and(|e| e.kind == "click" && e.value.as_deref() == Some("click_count=2"))
    });
    assert_eq!(
        after.last_event().value.as_deref(),
        Some("click_count=2"),
        "the app saw the wrong click count — two singles are not a double-click"
    );
}

/// Targeted delivery does not move the user's cursor.
///
/// This is THE test for the whole non-intrusive thesis of Wave 3: every event
/// below goes into the fixture's own event queue via `CGEvent.postToPid`, so the
/// physical pointer must be exactly where it was. (Do not touch the mouse while
/// this runs — it samples the real cursor.)
#[tokio::test]
#[ignore]
async fn tier_b_targeted_input_never_moves_the_real_cursor() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let state = fixture.state();

    let before: CursorPositionResult = bridge
        .call("input.cursor_position", serde_json::json!({}))
        .await
        .expect("input.cursor_position failed");

    let (px, py) = state.element("aleph.dragpad").frame.center();
    let _: serde_json::Value = bridge
        .call(
            "input.click",
            ClickParams {
                x: px,
                y: py,
                button: MouseButton::Left,
                pid: Some(state.pid),
                click_count: None,
            },
        )
        .await
        .expect("input.click failed");

    let (sx, sy) = state.element("aleph.scroll").frame.center();
    let _: serde_json::Value = bridge
        .call(
            "input.scroll",
            ScrollParams {
                direction: "down".to_string(),
                amount: 3,
                pid: Some(state.pid),
                x: Some(sx),
                y: Some(sy),
            },
        )
        .await
        .expect("input.scroll failed");

    let pad = state.element("aleph.dragpad").frame;
    let _: serde_json::Value = bridge
        .call(
            "input.drag",
            DragParams {
                start_x: pad.x + 20.0,
                start_y: pad.y + 20.0,
                end_x: pad.x + pad.width - 20.0,
                end_y: pad.y + pad.height - 20.0,
                duration_ms: Some(200),
                pid: Some(state.pid),
            },
        )
        .await
        .expect("input.drag failed");

    // The app must have actually received all that — otherwise "the cursor did not
    // move" would be trivially true because nothing happened at all.
    fixture.wait_until("the pad to report the drag", |s| {
        s.last_event.as_ref().is_some_and(|e| e.kind == "drag")
    });

    let after: CursorPositionResult = bridge
        .call("input.cursor_position", serde_json::json!({}))
        .await
        .expect("input.cursor_position failed");

    assert_eq!(
        (before.x, before.y),
        (after.x, after.y),
        "the targeted rail moved the user's physical cursor from {:?} to {:?}",
        (before.x, before.y),
        (after.x, after.y)
    );
}

/// A real drag walks a path.
///
/// `steps` is the number of intermediate `mouseDragged` events the app saw. A
/// down at the start and an up at the end — with nothing in between — is not a
/// drag; apps read it as a click at the start point.
#[tokio::test]
#[ignore]
async fn tier_b_drag_walks_the_path_between_the_endpoints() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let before = fixture.state();
    let pad = before.element("aleph.dragpad").frame;

    let result: serde_json::Value = bridge
        .call(
            "input.drag",
            DragParams {
                start_x: pad.x + 20.0,
                start_y: pad.y + 20.0,
                end_x: pad.x + pad.width - 20.0,
                end_y: pad.y + pad.height - 20.0,
                duration_ms: Some(300),
                pid: Some(before.pid),
            },
        )
        .await
        .expect("input.drag failed");
    assert_eq!(result["ok"], serde_json::json!(true));

    let after = fixture.wait_until("the pad to report a drag", |s| {
        s.last_event.as_ref().is_some_and(|e| e.kind == "drag")
    });
    let value = after
        .last_event()
        .value
        .as_deref()
        .expect("drag event carries no value");
    let steps: i64 = value
        .split(';')
        .find_map(|f| f.strip_prefix("steps="))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("drag event has no step count: {value}"));
    assert!(
        steps > 0,
        "the app saw a drag with no intermediate motion ({value}) — that is a click, not a drag"
    );
}

/// Typed text lands in the focused control, byte-identically, through the CGEvent
/// rail (a different code path from Tier A's `ax.set_value`).
#[tokio::test]
#[ignore]
async fn tier_b_type_text_lands_in_the_focused_field_byte_identically() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let before = fixture.state();

    // Precondition, asserted rather than assumed: a keyboard event posted to a pid
    // routes to that process's key-window focus, so this test is meaningless unless
    // the text field actually holds it. (The fixture takes focus itself at launch —
    // clicking the field would not do it, because the bridge's AX rail satisfies a
    // click on an NSTextField with AXConfirm, which does not move focus.)
    assert_eq!(
        before.focused.as_deref(),
        Some("aleph.textfield"),
        "the fixture's text field does not hold focus; typing would land nowhere"
    );

    let result: serde_json::Value = bridge
        .call(
            "input.type_text",
            TypeTextParams {
                text: UNICODE_PAYLOAD.to_string(),
                pid: Some(before.pid),
            },
        )
        .await
        .expect("input.type_text failed");
    assert_eq!(result["delivery"], serde_json::json!(DELIVERY_TARGETED));

    let after = fixture.wait_until("the typed text to arrive in the field", |s| {
        s.element("aleph.textfield").value.as_deref() == Some(UNICODE_PAYLOAD)
    });
    assert_eq!(
        after.element("aleph.textfield").value.as_deref(),
        Some(UNICODE_PAYLOAD),
        "typed text did not survive the CGEvent rail intact"
    );
}

/// Scrolling moves the scroll view — and moves it the way it was asked to.
#[tokio::test]
#[ignore]
async fn tier_b_scroll_moves_the_scroll_view() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let before = fixture.state();
    let baseline = scroll_offset(&before);
    let (x, y) = before.element("aleph.scroll").frame.center();

    let result: serde_json::Value = bridge
        .call(
            "input.scroll",
            ScrollParams {
                direction: "down".to_string(),
                amount: 5,
                pid: Some(before.pid),
                x: Some(x),
                y: Some(y),
            },
        )
        .await
        .expect("input.scroll failed");
    assert_eq!(result["ok"], serde_json::json!(true));

    // The document view is flipped, so scrolling DOWN increases the offset.
    let after = fixture.wait_until("the scroll view to scroll down", |s| {
        scroll_offset(s) > baseline
    });
    assert!(
        scroll_offset(&after) > baseline,
        "scroll offset did not increase: {baseline} → {}",
        scroll_offset(&after)
    );
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn scroll_offset(state: &FixtureState) -> f64 {
    state
        .element("aleph.scroll")
        .value
        .as_deref()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("scroll area has no numeric offset: {state:?}"))
}

fn find_by_title<'a>(root: &'a AxElement, title: &str) -> Option<&'a AxElement> {
    if root.title.as_deref() == Some(title) {
        return Some(root);
    }
    root.children.iter().find_map(|c| find_by_title(c, title))
}
