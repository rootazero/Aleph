//! Pure wire contracts for capability discovery and future terminal/UI owners.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const MAX_PTY_DIMENSION: u16 = 1_000;
pub const MAX_PTY_INPUT_BYTES: usize = 65_536;
pub const MAX_UI_TEXT_BYTES: usize = 65_536;
pub const MAX_UI_KEY_BYTES: usize = 256;
pub const MAX_UI_QUEUE: usize = 128;
pub const UI_ACK_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDescribeParams {
    #[serde(default)]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDescription {
    pub descriptor: serde_json::Value,
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PtySpawnParams {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub rows: u16,
    #[serde(default)]
    pub cols: u16,
}

/// Compatibility name for the legacy `pty.spawn` handler.
///
/// The wire type is owned by the shared protocol crate; Core only re-exports
/// this alias so old callers keep the same serde defaults and field names.
pub type SpawnParams = PtySpawnParams;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum PtyToolParams {
    Spawn {
        params: PtySpawnParams,
    },
    // A struct variant makes serde enforce deny_unknown_fields on this tag.
    List {},
    Input {
        session_id: String,
        data: String,
        #[serde(default)]
        base64: bool,
    },
    Attach {
        session_id: String,
    },
    Resize {
        session_id: String,
        rows: u16,
        cols: u16,
    },
    Close {
        session_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PtyInvokeParams {
    pub session_id: String,
    pub operation: PtyOperation,
}

impl PtyInvokeParams {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.session_id.is_empty() {
            return Err("session_id must not be empty");
        }
        self.operation.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum PtyOperation {
    Input {
        #[schemars(
            description = "UTF-8 byte length must be at most 65536; character count is not the limit"
        )]
        data: String,
        #[serde(default)]
        base64: bool,
    },
    Attach,
    Resize {
        #[schemars(range(min = 1, max = 1000))]
        rows: u16,
        #[schemars(range(min = 1, max = 1000))]
        cols: u16,
    },
    Close,
}

impl PtyOperation {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Input { data, .. } if data.len() > MAX_PTY_INPUT_BYTES => {
                Err("PTY input exceeds maximum UTF-8 byte length")
            }
            Self::Resize { rows, cols }
                if *rows == 0
                    || *cols == 0
                    || *rows > MAX_PTY_DIMENSION
                    || *cols > MAX_PTY_DIMENSION =>
            {
                Err("PTY dimensions are out of range")
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiAttachParams {
    pub session_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiAttachment {
    pub surface_id: String,
    pub mount_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiInvokeParams {
    pub surface_id: String,
    pub mount_generation: u64,
    pub operation: UiOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiOperation {
    Display {
        #[schemars(
            description = "UTF-8 byte length must be at most 65536; character count is not the limit"
        )]
        text: String,
    },
    // Unit variants discard tagged payload fields instead of rejecting them.
    Input {},
}

impl UiInvokeParams {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.surface_id.is_empty() {
            return Err("surface_id must not be empty");
        }
        if self.mount_generation == 0 {
            return Err("mount_generation must be non-zero");
        }
        self.operation.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiDisplayMessage {
    pub surface_id: String,
    pub mount_generation: u64,
    pub request_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiRenderAck {
    pub surface_id: String,
    pub mount_generation: u64,
    pub request_id: String,
    pub rendered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiSubmitInput {
    pub surface_id: String,
    pub mount_generation: u64,
    pub event: UiInputEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiInputEvent {
    KeyDown { key: String },
    MouseDown { x: i32, y: i32, button: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiDetachParams {
    pub surface_id: String,
    pub mount_generation: u64,
}

impl UiOperation {
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Self::Display { text } = self {
            if text.len() > MAX_UI_TEXT_BYTES {
                return Err("display text exceeds maximum UTF-8 byte length");
            }
        }
        Ok(())
    }
}

impl UiInputEvent {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::KeyDown { key } if key.len() > MAX_UI_KEY_BYTES => {
                Err("key exceeds maximum UTF-8 byte length")
            }
            Self::MouseDown { button, .. } if *button > 4 => Err("mouse button is out of range"),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_list_rejects_unknown_fields_and_preserves_roundtrip() {
        let valid = json!({"operation": "list"});
        let parsed: PtyToolParams = serde_json::from_value(valid.clone()).expect("valid List");
        assert_eq!(serde_json::to_value(parsed).expect("List wire"), valid);
        assert!(serde_json::from_value::<PtyToolParams>(
            json!({"operation": "list", "unexpected": true}),
        )
        .is_err());
    }

    #[test]
    fn wire_ui_input_rejects_unknown_fields_and_preserves_roundtrip() {
        let valid = json!({"surface_id": "s", "mount_generation": 1,
            "operation": {"operation": "input"}});
        let parsed: UiInvokeParams = serde_json::from_value(valid.clone()).expect("valid UI Input");
        assert_eq!(serde_json::to_value(parsed).expect("UI Input wire"), valid);
        assert!(serde_json::from_value::<UiInvokeParams>(json!({
            "surface_id": "s", "mount_generation": 1,
            "operation": {"operation": "input", "unexpected": true}
        }))
        .is_err());
    }
}
