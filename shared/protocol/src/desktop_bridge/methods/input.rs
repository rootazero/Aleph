//! Coordinate-space input schemas for the desktop bridge.
//!
//! Two delivery rails share these types:
//!
//! - **targeted** — the event is built against a session-state event source and
//!   posted straight into the target process's event queue (`pid` present).
//!   The user's real cursor never moves and the target app need not be
//!   frontmost.
//! - **global** — the event is posted to the global HID event tap. It
//!   physically drags the user's cursor and only lands where the frontmost app
//!   is the intended one.
//!
//! `pid` is a *request* for the targeted rail, never a guarantee: a platform
//! that cannot target falls back to the global tap. Every result therefore
//! carries `delivery`, so the model can SEE which rail actually ran instead of
//! having to guess — the honesty lives in the payload, never in a harness rule
//! (R9).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const METHOD_CLICK: &str = "input.click";
pub const METHOD_DOUBLE_CLICK: &str = "input.double_click";
pub const METHOD_TYPE_TEXT: &str = "input.type_text";
pub const METHOD_KEY_COMBO: &str = "input.key_combo";
pub const METHOD_KEY_BUTTON: &str = "input.key_button";
pub const METHOD_SCROLL: &str = "input.scroll";
pub const METHOD_DRAG: &str = "input.drag";
pub const METHOD_HOVER: &str = "input.hover";
pub const METHOD_CURSOR_POSITION: &str = "input.cursor_position";
pub const METHOD_MOUSE_BUTTON: &str = "input.mouse_button";

// ── Deadlines ────────────────────────────────────────────────────────────────

/// Deadline for a synthesized input event.
///
/// Posting an event into a process's queue is fire-and-forget: the helper builds
/// the `CGEvent` and hands it over without waiting for the app to consume it, so
/// these calls are bounded by the helper's own work, not by the target app's
/// responsiveness.
pub const DEFAULT_TIMEOUT_MS: u64 = 2_000;

/// Deadline for the two click methods.
///
/// They are the exception to the rule above: before falling back to a
/// coordinate event they walk the target's accessibility subtree looking for a
/// control that can be pressed directly, and every node in that walk is a round
/// trip into the app.
pub const TIMEOUT_MS_CLICK: u64 = 5_000;

pub const TIMEOUT_OVERRIDES_MS: &[(&str, u64)] = &[
    (METHOD_CLICK, TIMEOUT_MS_CLICK),
    (METHOD_DOUBLE_CLICK, TIMEOUT_MS_CLICK),
];

/// `delivery` value: posted into the target process's own event queue; the
/// user's cursor never moved and the app did not need to be frontmost.
pub const DELIVERY_TARGETED: &str = "targeted";
/// `delivery` value: posted to the global HID event tap; the user's cursor was
/// physically moved and the frontmost app received the event.
pub const DELIVERY_GLOBAL: &str = "global";

/// A helper that omits `delivery` is one that only has the global tap — the
/// pessimistic reading is the correct one, and it is never silently upgraded.
fn default_delivery() -> String {
    DELIVERY_GLOBAL.to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PressAction {
    Press,
    Release,
    Click,
}

/// Params for `input.click` **and** `input.double_click`.
///
/// `input.double_click` ignores `click_count` and always delivers a native
/// double-click (click count 2).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClickParams {
    pub x: f64,
    pub y: f64,
    pub button: MouseButton,
    /// Target process for the targeted rail; absent ⇒ global HID tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Native click count carried on the event itself. Absent or `1` is a
    /// single click; `2` is a real double-click. Two independent single clicks
    /// are NOT a double-click — the app reads the click count off the event, so
    /// it must be set on the event rather than synthesized by clicking twice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub click_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClickResult {
    pub ok: bool,
    /// Which rail actually delivered the event: [`DELIVERY_TARGETED`] or
    /// [`DELIVERY_GLOBAL`].
    #[serde(default = "default_delivery")]
    pub delivery: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TypeTextParams {
    pub text: String,
    /// Target process for the targeted rail; absent ⇒ global HID tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TypeTextResult {
    pub ok: bool,
    #[serde(default = "default_delivery")]
    pub delivery: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyComboParams {
    pub modifiers: Vec<String>,
    pub key: String,
    /// Target process for the targeted rail; absent ⇒ global HID tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyComboResult {
    pub ok: bool,
    #[serde(default = "default_delivery")]
    pub delivery: String,
}

/// Hold a key/chord down, or let it back up, without the atomic
/// press-and-release [`KeyComboParams`] performs.
///
/// This exists because a held key is the one input that outlives the call that
/// made it: `key_combo` is over when it returns, `key_button` leaves a physical
/// key down until something releases it. Its own method — rather than a flag on
/// `key_combo` — keeps that difference visible at the wire, and lets the held
/// key ride the same targeted rail its release will have to ride.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyButtonParams {
    /// The chord, e.g. `["cmd", "shift"]` or `["a"]`. Modifiers and ordinary
    /// keys are not separated here — the whole chord goes down or up together.
    pub keys: Vec<String>,
    pub action: PressAction,
    /// Target process for the targeted rail; absent ⇒ global HID tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyButtonResult {
    pub ok: bool,
    #[serde(default = "default_delivery")]
    pub delivery: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScrollParams {
    pub direction: String,
    pub amount: i32,
    /// Target process for the targeted rail; absent ⇒ global HID tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Point the scroll event is stamped with, in global screen points.
    ///
    /// The global rail scrolls wherever the cursor already is, so it may omit
    /// these. The targeted rail must NOT: the cursor never moves, so the event
    /// carries the only location the app can route the scroll by. Absent on the
    /// targeted rail ⇒ the app decides, which is almost never what was meant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScrollResult {
    pub ok: bool,
    #[serde(default = "default_delivery")]
    pub delivery: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DragParams {
    pub start_x: f64,
    pub start_y: f64,
    pub end_x: f64,
    pub end_y: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Target process for the targeted rail; absent ⇒ global HID tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DragResult {
    pub ok: bool,
    #[serde(default = "default_delivery")]
    pub delivery: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HoverParams {
    pub x: f64,
    pub y: f64,
    /// Target process for the targeted rail; absent ⇒ global HID tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HoverResult {
    pub ok: bool,
    #[serde(default = "default_delivery")]
    pub delivery: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CursorPositionResult {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MouseButtonParams {
    pub x: f64,
    pub y: f64,
    pub button: MouseButton,
    pub action: PressAction,
    /// Target process for the targeted rail; absent ⇒ global HID tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MouseButtonResult {
    pub ok: bool,
    #[serde(default = "default_delivery")]
    pub delivery: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_params_roundtrip_and_lowercase_button() {
        let p = ClickParams {
            x: 10.0,
            y: 20.0,
            button: MouseButton::Left,
            pid: None,
            click_count: None,
        };
        let j = serde_json::to_string(&p).unwrap();
        // Verify "left" (not "Left") in the JSON
        assert!(
            j.contains("\"button\":\"left\""),
            "expected lowercase button, got: {j}"
        );
        let back: ClickParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.button, MouseButton::Left);
        assert_eq!(back.x, 10.0);
        assert_eq!(back.y, 20.0);
    }

    #[test]
    fn click_params_omit_targeting_fields_when_absent() {
        // The pre-targeting wire must stay byte-identical, so an old helper that
        // knows nothing of `pid` / `click_count` sees exactly what it saw before.
        let p = ClickParams {
            x: 1.0,
            y: 2.0,
            button: MouseButton::Left,
            pid: None,
            click_count: None,
        };
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(j, r#"{"x":1.0,"y":2.0,"button":"left"}"#);
    }

    #[test]
    fn click_params_carry_pid_and_click_count() {
        let j = r#"{"x":1.0,"y":2.0,"button":"left","pid":4242,"click_count":2}"#;
        let p: ClickParams = serde_json::from_str(j).unwrap();
        assert_eq!(p.pid, Some(4242));
        assert_eq!(p.click_count, Some(2));
    }

    #[test]
    fn key_combo_params_roundtrip() {
        let p = KeyComboParams {
            modifiers: vec!["cmd".into(), "shift".into()],
            key: "s".into(),
            pid: Some(7),
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: KeyComboParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.modifiers, vec!["cmd", "shift"]);
        assert_eq!(back.key, "s");
        assert_eq!(back.pid, Some(7));
    }

    #[test]
    fn mouse_button_params_roundtrip_with_press_action() {
        let p = MouseButtonParams {
            x: 5.0,
            y: 15.0,
            button: MouseButton::Right,
            action: PressAction::Press,
            pid: None,
        };
        let j = serde_json::to_string(&p).unwrap();
        assert!(
            j.contains("\"action\":\"press\""),
            "expected lowercase action, got: {j}"
        );
        let back: MouseButtonParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.button, MouseButton::Right);
        assert_eq!(back.action, PressAction::Press);
    }

    #[test]
    fn scroll_params_carry_target_point() {
        let p = ScrollParams {
            direction: "down".into(),
            amount: 3,
            pid: Some(99),
            x: Some(400.0),
            y: Some(300.0),
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: ScrollParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.pid, Some(99));
        assert_eq!(back.x, Some(400.0));
        assert_eq!(back.y, Some(300.0));
    }

    #[test]
    fn results_report_the_rail_that_actually_ran() {
        let r: ClickResult = serde_json::from_str(r#"{"ok":true,"delivery":"targeted"}"#).unwrap();
        assert!(r.ok);
        assert_eq!(r.delivery, DELIVERY_TARGETED);
    }

    #[test]
    fn result_without_delivery_reads_as_global_not_targeted() {
        // A helper that never learned about targeting only has the global tap.
        // Absence must never be optimistically read as "the cursor stayed put".
        let r: ScrollResult = serde_json::from_str(r#"{"ok":true}"#).unwrap();
        assert_eq!(r.delivery, DELIVERY_GLOBAL);
    }
}
