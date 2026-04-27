use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const METHOD_CLICK: &str = "input.click";
pub const METHOD_DOUBLE_CLICK: &str = "input.double_click";
pub const METHOD_TYPE_TEXT: &str = "input.type_text";
pub const METHOD_KEY_COMBO: &str = "input.key_combo";
pub const METHOD_SCROLL: &str = "input.scroll";
pub const METHOD_DRAG: &str = "input.drag";
pub const METHOD_HOVER: &str = "input.hover";
pub const METHOD_CURSOR_POSITION: &str = "input.cursor_position";
pub const METHOD_MOUSE_BUTTON: &str = "input.mouse_button";
pub const METHOD_CLIPBOARD_READ: &str = "input.clipboard_read";
pub const METHOD_CLIPBOARD_WRITE: &str = "input.clipboard_write";
pub const SUGGESTED_TIMEOUT_MS: u64 = 2_000;

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClickParams {
    pub x: f64,
    pub y: f64,
    pub button: MouseButton,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClickResult {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TypeTextParams {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TypeTextResult {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyComboParams {
    pub modifiers: Vec<String>,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyComboResult {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScrollParams {
    pub direction: String,
    pub amount: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScrollResult {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DragParams {
    pub start_x: f64,
    pub start_y: f64,
    pub end_x: f64,
    pub end_y: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DragResult {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HoverParams {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HoverResult {
    pub ok: bool,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MouseButtonResult {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardReadResult {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardWriteParams {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardWriteResult {
    pub ok: bool,
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
    fn key_combo_params_roundtrip() {
        let p = KeyComboParams {
            modifiers: vec!["cmd".into(), "shift".into()],
            key: "s".into(),
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: KeyComboParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.modifiers, vec!["cmd", "shift"]);
        assert_eq!(back.key, "s");
    }

    #[test]
    fn mouse_button_params_roundtrip_with_press_action() {
        let p = MouseButtonParams {
            x: 5.0,
            y: 15.0,
            button: MouseButton::Right,
            action: PressAction::Press,
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
}
