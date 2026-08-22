//! Closed-loop computer-use e2e: Rust `SwiftBridge` → real `AlephBridge` → real
//! macOS (AX + CGEvent) → a real app, `AlephFixture`, which reports back what
//! actually happened to it.
//!
//! Run with `just test-computer-use-e2e`. Everything here is `#[ignore]`: it needs
//! Accessibility (TCC) and, for Tier B, a real logged-in GUI session.
//!
//! Whole file is macOS-only: it drives the real `AlephBridge` helper via the
//! macOS-only `aleph-desktop-macos` crate. On any other host the integration
//! test compiles to an empty binary, which `cargo check --workspace` treats as
//! "0 tests" — no failure.
#![cfg(target_os = "macos")]

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
//! **Tier B — background input rail, visible.** The background rail that actually
//! works on macOS is keyboard synthesis (`type_text`) plus the AX click ladder —
//! both `postToPid`-delivered and cursor-free. A *synthesized mouse* event posted
//! to a pid, by contrast, is never routed to a window, so scroll / drag / a
//! coordinate click with no AX rung have NO background delivery and the bridge
//! refuses them (`-32050`) rather than post-and-hope. Tier B runs the fixture
//! visible — on screen, in an active GUI session — because that is the setting
//! where a naive mouse rail would look like it worked, and it proves the working
//! ops land AND the refused ops still refuse. It also samples the real cursor to
//! pin the non-intrusive thesis: the rail that DOES work never moves it.
//!
//! Tier A and Tier B must never be merged. Tier A's off-screen window has no
//! business receiving a keyboard-focus or cursor assertion, and Tier B's visible
//! window is the only place the "did the app act, and did the cursor stay put"
//! questions can be answered honestly.

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aleph_desktop::bridge::client::SwiftBridge;
use aleph_protocol::desktop_bridge::methods::ax::{
    AxActionResult, AxElement, AxLocator, PerformActionParams, QueryByRoleParams,
    QueryFocusedParams, QueryListResult, QueryResult, QueryTreeParams, SetValueParams,
    DEFAULT_MAX_NODES,
};
use aleph_protocol::desktop_bridge::methods::input::{
    ClickParams, CursorPositionResult, DragParams, MouseButton, ScrollParams, TypeTextParams,
    DELIVERY_TARGETED,
};
use serde::{Deserialize, Serialize};

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
    // Deserialized for completeness; the click-count / drag-step payloads it once
    // carried were asserted by the synthesized-mouse tests, which no longer exist
    // (that rail does not deliver in the background — see the refusal tests).
    #[allow(dead_code)]
    value: Option<String>,
    #[allow(dead_code)]
    seq: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct FixtureState {
    pid: i32,
    // Part of the wire format and shown in `{:?}` failure dumps; no assertion
    // reads it directly since `wait_until` polls observed facts, not the counter.
    #[allow(dead_code)]
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

/// Warm the login session's global accessibility state, once per test binary.
///
/// macOS serves a degraded, window-less AX tree to a third-party client — the
/// application element reports *itself* as its only window (a self-cycle), with
/// no `AXWindow` anywhere — until some assistive client "announces" itself and
/// flips a login-session-global switch. From then on every app (including ones
/// launched afterwards) vends its real tree, to every client, including cold
/// queries. Merely being trusted and calling `AXUIElementCopyAttributeValue` is
/// not the announcement; System Events touching a process's UI-element tree IS,
/// so poking it once here warms the switch for the whole run.
///
/// Best-effort: on a machine where Automation (to System Events) is not granted
/// the poke fails and returns — and the cold tree then trips the explicit
/// "off-display / accessibility not granted" assertion below, which says what to
/// do. Without this, a suite run on a freshly-booted session where nothing has
/// yet announced itself fails for a reason that has nothing to do with the code
/// under test. See `project-computer-use-runtime-qa-macos27` for the full trace.
fn ensure_accessibility_warm() {
    static WARM: std::sync::Once = std::sync::Once::new();
    WARM.call_once(|| {
        let _ = Command::new("osascript")
            .args([
                "-e",
                "tell application \"System Events\" to get name of first process",
            ])
            .output();
    });
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
        // Flip the session-global accessibility switch before anything is queried
        // (see `ensure_accessibility_warm`), or a cold session returns a
        // window-less self-cyclic tree and every AX assertion fails spuriously.
        ensure_accessibility_warm();

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

/// Assert a bridge call was refused with the background-mouse-undeliverable error
/// (`-32050`), and hand back the message so a test can also check the alternative
/// it names. Any `Ok` — a "success" for a mouse op that has no background rail —
/// is the exact lie this suite exists to catch, so it is a failure here.
async fn assert_refused(bridge: &SwiftBridge, method: &str, params: impl Serialize) -> String {
    let err = bridge
        .call::<_, serde_json::Value>(method, params)
        .await
        .expect_err(&format!(
            "{method} must refuse (no background mouse rail on macOS), not report success"
        ));
    let msg = err.to_string();
    assert!(
        msg.contains("-32050"),
        "{method} was refused with the wrong error (expected -32050 background-undeliverable): {msg}"
    );
    msg
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
                max_nodes: DEFAULT_MAX_NODES,
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
                max_nodes: DEFAULT_MAX_NODES,
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

/// A walk that runs out of budget says so, and a walk that does not says that
/// too.
///
/// The budget itself is easy to get right and easy to get away with getting
/// wrong: a helper that stops early and reports nothing looks, from the calling
/// side, exactly like an application with a small UI. This is asserted against a
/// fixture whose tree is *known* to be bigger than three nodes, so "truncated"
/// and "that is genuinely all of it" are distinguishable here in a way they are
/// not against an arbitrary app.
#[tokio::test]
#[ignore]
async fn tier_a_a_budget_exhausted_walk_reports_that_it_was_cut() {
    let fixture = Fixture::launch(Mode::Headless);
    let bridge = bridge();
    let state = fixture.state();

    let cut: QueryResult = bridge
        .call(
            "ax.query_tree",
            QueryTreeParams {
                pid: Some(state.pid),
                max_depth: 10,
                max_nodes: 3,
            },
        )
        .await
        .expect("ax.query_tree failed");

    assert!(
        cut.truncated,
        "a 3-node budget over the fixture's tree must report truncation"
    );
    assert_eq!(
        cut.node_count, 3,
        "the walk must spend exactly its budget, not approximately"
    );

    let whole: QueryResult = bridge
        .call(
            "ax.query_tree",
            QueryTreeParams {
                pid: Some(state.pid),
                max_depth: 10,
                max_nodes: DEFAULT_MAX_NODES,
            },
        )
        .await
        .expect("ax.query_tree failed");

    assert!(
        !whole.truncated,
        "the fixture's whole tree fits in the default budget; reporting truncation \
         here would make the flag useless"
    );
    assert!(
        whole.node_count > cut.node_count,
        "the unbudgeted walk must have read more than the capped one \
         ({} vs {})",
        whole.node_count,
        cut.node_count
    );
}

/// `ax.query_focused` answers for the process it is asked about, not for the
/// desktop.
///
/// This is the property the `type_text` focus gate depends on and did not have.
/// The fixture is an `.accessory` app parked off every display: it is emphatically
/// **not** frontmost, which is precisely the situation the targeted input rail
/// runs in. The system-wide answer therefore belongs to whatever the user has in
/// front of them — and a gate reading it is inspecting a window the keystrokes
/// will never reach.
#[tokio::test]
#[ignore]
async fn tier_a_focused_element_is_answered_per_process_not_per_desktop() {
    let fixture = Fixture::launch(Mode::Headless);
    let bridge = bridge();
    let state = fixture.state();

    let mine: QueryResult = bridge
        .call(
            "ax.query_focused",
            QueryFocusedParams {
                pid: Some(state.pid),
            },
        )
        .await
        .expect("ax.query_focused(pid) failed");

    let element = mine
        .element
        .expect("the fixture focuses a control at launch, so it must report one");
    assert_eq!(
        element.pid, state.pid,
        "asked about pid {} and got an element owned by pid {} — the contract is \
         that the answer belongs to the process asked about, or there is no answer",
        state.pid, element.pid
    );

    // And the system-wide question is a different one: whatever it returns, it is
    // not the off-screen accessory app nobody is driving.
    let system: QueryResult = bridge
        .call("ax.query_focused", QueryFocusedParams::default())
        .await
        .expect("ax.query_focused() failed");
    if let Some(el) = system.element {
        assert_ne!(
            el.pid, state.pid,
            "an off-screen .accessory app must not be holding the SYSTEM focus; if it \
             is, this test can no longer tell the two questions apart"
        );
    }
}

/// The fixture's own geometry and the bridge's AX bounds agree.
///
/// Tier B aims real clicks using the frames the FIXTURE publishes, so the two
/// coordinate spaces (AppKit's bottom-left vs AX/CGEvent's top-left) must line up.
/// If they ever drift — a Retina scale factor creeping in, a flip against the
/// wrong screen — Tier B would start missing its targets for a reason that has
/// nothing to do with the input rail. Pin it here, where the failure is legible.
///
/// The comparison is on the CENTER point, not the edges, and that is deliberate.
/// AppKit reports an `NSButton`'s AX position/size as its *alignment* rect — the
/// visual control inside its bezel — while the fixture publishes the view's full
/// frame, so the two disagree by the bezel inset (≈6pt in x, 12pt in width) on a
/// bezeled control. That inset is symmetric, so the CENTER still agrees to within
/// a point — and the center is exactly what Tier B targets (`frame.center()`), so
/// it is the coordinate whose agreement actually matters. A real coordinate-space
/// drift (a 2× Retina scale, a wrong-screen flip) throws the center off by
/// hundreds of points, so this still catches the failure it was written to catch.
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
                max_nodes: DEFAULT_MAX_NODES,
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

    let ax_center = (ax.x + ax.width / 2.0, ax.y + ax.height / 2.0);
    let own_center = own.center();
    for (what, ax_v, own_v) in [
        ("center x", ax_center.0, own_center.0),
        ("center y", ax_center.1, own_center.1),
    ] {
        assert!(
            (ax_v - own_v).abs() <= 1.0,
            "{what}: the bridge's AX center ({ax_v}) and the fixture's own frame center \
             ({own_v}) disagree by more than a point — the two coordinate spaces have drifted"
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

/// A double-click has no background rail on macOS, so the bridge refuses it.
///
/// A multi-click's whole payload is the click COUNT carried on a synthesized
/// mouse event, and `AXPress` carries no count — so a double-click can never take
/// the AX rung, and the synthesized-mouse rail it would need does not deliver in
/// the background here. The old bridge posted a `postToPid` multi-click the app
/// never acted on and returned `ok:true`; the contract now is an honest refusal.
#[tokio::test]
#[ignore]
async fn tier_b_double_click_refuses_there_is_no_background_multi_click() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let before = fixture.state();
    let (x, y) = before.element("aleph.dragpad").frame.center();

    assert_refused(
        &bridge,
        "input.double_click",
        ClickParams {
            x,
            y,
            button: MouseButton::Left,
            pid: Some(before.pid),
            click_count: None,
        },
    )
    .await;

    // A refusal that still moved app state would be its own lie.
    std::thread::sleep(Duration::from_millis(300));
    let after = fixture.state();
    assert_eq!(
        after.last_event().kind,
        "ready",
        "a refused double-click must not have reached the app: {after:?}"
    );
}

/// A coordinate click that finds no AX rung refuses rather than silently doing
/// nothing.
///
/// The drag pad advertises no press action, so a click on it has no background
/// rail. The old bridge posted a `postToPid` mouse event the app never acted on
/// and returned `ok:true` — a click that did nothing while claiming it had, which
/// is the exact lie this file exists to catch. The contract now: refuse with
/// -32050, name the element rail as the alternative, and leave the app untouched.
#[tokio::test]
#[ignore]
async fn tier_b_coordinate_click_with_no_ax_rung_refuses_rather_than_lying() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let before = fixture.state();
    let (x, y) = before.element("aleph.dragpad").frame.center();

    let msg = assert_refused(
        &bridge,
        "input.click",
        ClickParams {
            x,
            y,
            button: MouseButton::Left,
            pid: Some(before.pid),
            click_count: None,
        },
    )
    .await;
    assert!(
        msg.contains("ax_action"),
        "the click refusal should route the model to the element rail: {msg}"
    );

    std::thread::sleep(Duration::from_millis(300));
    let after = fixture.state();
    assert_eq!(
        after.last_event().kind,
        "ready",
        "a refused click must not have reached the app: {after:?}"
    );
}

/// The background rail that WORKS never moves the user's cursor.
///
/// The non-intrusive thesis, scoped to what macOS actually delivers in the
/// background: an AX-rung click and a typed string. Both reach the target process
/// without the window server placing the pointer, so the physical cursor must be
/// exactly where it was. (Do not touch the mouse while this runs — it samples the
/// real cursor.) The synthesized-mouse rail is deliberately not exercised: it
/// does not run at all (see the refusal tests), so a cursor that "did not move"
/// because nothing happened would be a vacuous pass. Here things DO happen — the
/// counter ticks and the field fills — and the cursor still does not move.
#[tokio::test]
#[ignore]
async fn tier_b_the_working_background_rail_never_moves_the_cursor() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let state = fixture.state();

    let before: CursorPositionResult = bridge
        .call("input.cursor_position", serde_json::json!({}))
        .await
        .expect("input.cursor_position failed");

    // An AX-rung click on the button — background, cursor-free.
    let (bx, by) = state.element("aleph.button").frame.center();
    let click: serde_json::Value = bridge
        .call(
            "input.click",
            ClickParams {
                x: bx,
                y: by,
                button: MouseButton::Left,
                pid: Some(state.pid),
                click_count: None,
            },
        )
        .await
        .expect("AX-rung click failed");
    assert_eq!(
        click["path"],
        serde_json::json!("accessibility"),
        "the button click should take the AX rung: {click}"
    );

    // A typed string — background, cursor-free (a different code path).
    let _: serde_json::Value = bridge
        .call(
            "input.type_text",
            TypeTextParams {
                text: UNICODE_PAYLOAD.to_string(),
                pid: Some(state.pid),
            },
        )
        .await
        .expect("input.type_text failed");

    // Both really happened — otherwise "the cursor did not move" is vacuous.
    fixture.wait_until("the button press and the typed text to land", |s| {
        s.counter == state.counter + 1
            && s.element("aleph.textfield").value.as_deref() == Some(UNICODE_PAYLOAD)
    });

    let after: CursorPositionResult = bridge
        .call("input.cursor_position", serde_json::json!({}))
        .await
        .expect("input.cursor_position failed");
    assert_eq!(
        (before.x, before.y),
        (after.x, after.y),
        "the background rail moved the user's physical cursor from {:?} to {:?}",
        (before.x, before.y),
        (after.x, after.y)
    );
}

/// A drag has no background equivalent on macOS, so the bridge refuses it.
///
/// An app tracks a drag by the `mouseDragged` motion between the endpoints, and
/// that motion is exactly what the window server never associates with a
/// `postToPid` event — there is no synthesized-mouse delivery to walk a path
/// with. So the rail refuses rather than post a path nothing receives.
#[tokio::test]
#[ignore]
async fn tier_b_drag_refuses_there_is_no_background_drag() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let before = fixture.state();
    let pad = before.element("aleph.dragpad").frame;

    assert_refused(
        &bridge,
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
    .await;
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

/// A background scroll has no delivery on macOS; the bridge refuses it and points
/// the model at the keyboard rail, which DOES reach the focused view in the
/// background (PageDown/Space via `key_combo`).
#[tokio::test]
#[ignore]
async fn tier_b_scroll_refuses_and_points_to_the_keyboard() {
    let fixture = Fixture::launch(Mode::Visible);
    let bridge = bridge();
    let before = fixture.state();
    let (x, y) = before.element("aleph.scroll").frame.center();

    let msg = assert_refused(
        &bridge,
        "input.scroll",
        ScrollParams {
            direction: "down".to_string(),
            amount: 5,
            pid: Some(before.pid),
            x: Some(x),
            y: Some(y),
        },
    )
    .await;
    assert!(
        msg.contains("key_combo"),
        "the scroll refusal should route the model to the keyboard rail: {msg}"
    );
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn find_by_title<'a>(root: &'a AxElement, title: &str) -> Option<&'a AxElement> {
    if root.title.as_deref() == Some(title) {
        return Some(root);
    }
    root.children.iter().find_map(|c| find_by_title(c, title))
}
