//! Wire contract for the paired Bash JSONL profile.

use serde_json::{json, Value};

pub(crate) const PROFILE: &str = "jsonl_inspect";
const WRITE: &str = "write_stdin";
const CLOSE: &str = "close_stdin";

pub(crate) fn is_private(name: &str, args: &Value) -> bool {
    if args.get("stdin_profile").and_then(Value::as_str) == Some(PROFILE)
        || input_action(args).is_some()
    {
        return true;
    }
    name.eq_ignore_ascii_case("bash")
        && (!args.is_object()
            || args.get("stdin_profile").is_some()
            || args.get("record").is_some())
}

fn input_action(args: &Value) -> Option<&'static str> {
    match args.get("process_action").and_then(Value::as_str) {
        Some(WRITE) => Some(WRITE),
        Some(CLOSE) => Some(CLOSE),
        _ => None,
    }
}

/// The public view also covers malformed combinations. Copy only validated
/// discriminants and numbers, never a field supplied as arbitrary text.
pub(crate) fn project_arguments(name: &str, args: &Value) -> Option<Value> {
    if !is_private(name, args) {
        return None;
    }
    let mut projected = json!({ "redacted": true });
    if let Some(action) = input_action(args) {
        projected["process_action"] = json!(action);
    }
    if let Some(id) = args.get("process_id").and_then(Value::as_u64) {
        projected["process_id"] = json!(id);
    }
    if args.get("stdin_profile").and_then(Value::as_str) == Some(PROFILE) {
        projected["stdin_profile"] = json!(PROFILE);
    }
    Some(projected)
}

#[cfg(target_os = "linux")]
use crate::sandbox::shell_input::{JsonlFrame, ShellEffectDenied};

#[cfg(target_os = "linux")]
pub(crate) enum Operation {
    Launch {
        background: bool,
        yield_ms: Option<u64>,
    },
    Write {
        id: u64,
        frame: JsonlFrame,
    },
    Close {
        id: u64,
    },
}

#[cfg(target_os = "linux")]
impl Operation {
    pub(crate) fn parse(args: Value) -> Result<Self, &'static str> {
        const INVALID: &str = "invalid arguments for the fixed JSONL profile";
        let fields = args.as_object().ok_or(INVALID)?;
        let id = fields.get("process_id").and_then(Value::as_u64);
        match (input_action(&args), id) {
            (Some(WRITE), Some(id)) if fields.len() == 3 => {
                let frame = JsonlFrame::from_record(fields.get("record").ok_or(INVALID)?).map_err(
                    |_: ShellEffectDenied| {
                        "one compact JSONL frame must fit in 4096 bytes including its newline"
                    },
                )?;
                return Ok(Self::Write { id, frame });
            }
            (Some(CLOSE), Some(id)) if fields.len() == 2 => return Ok(Self::Close { id }),
            _ => {}
        }
        if fields.get("stdin_profile").and_then(Value::as_str) != Some(PROFILE)
            || fields
                .keys()
                .any(|name| !matches!(name.as_str(), "stdin_profile" | "background" | "yield_ms"))
        {
            return Err(INVALID);
        }
        let background = match fields.get("background") {
            Some(value) => value.as_bool().ok_or(INVALID)?,
            None => false,
        };
        let yield_ms = match fields.get("yield_ms") {
            Some(value) => Some(
                value
                    .as_u64()
                    .filter(|ms| (1..=10_000).contains(ms))
                    .ok_or(INVALID)?,
            ),
            None => None,
        };
        if !background && yield_ms.is_none() {
            return Err(INVALID);
        }
        Ok(Self::Launch {
            background,
            yield_ms,
        })
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn extend_definition(definition: &mut crate::tools::service::ToolDefinition) {
    let Some(properties) = definition
        .input_schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    properties.insert(
        "stdin_profile".into(),
        json!({"type": "string", "enum": [PROFILE]}),
    );
    properties.insert(
        "record".into(),
        json!({"description": "One JSON value, including null. Used only with write_stdin."}),
    );
    definition.description.push_str(" On this paired Linux route, start the fixed jq data inspector with stdin_profile: jsonl_inspect and background: true or yield_ms. Supply only those launch fields. To deliver one JSON value use process_action: write_stdin with process_id and record. To end input use process_action: close_stdin with process_id only. Each write and first EOF require operator confirmation. Read results with process_action: read.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shell_jsonl_projection_preserves_only_trusted_fields() {
        let raw = json!({
            "process_action": "write_stdin", "process_id": 42,
            "record": "PRIVATE_INPUT_CANARY", "cmd": "PRIVATE_COMMAND_CANARY"
        });
        let projected = project_arguments("bash", &raw).unwrap();
        assert_eq!(projected["process_action"], "write_stdin");
        assert_eq!(projected["process_id"], 42);
        assert_eq!(projected["redacted"], true);
        assert!(!projected.to_string().contains("CANARY"));
        assert!(project_arguments("bash", &json!({"cmd": "printf ok"})).is_none());
        assert!(project_arguments("another_tool", &json!({"record": 1})).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shell_jsonl_input_distinguishes_null_and_rejects_overrides() {
        let write = json!({"process_action":"write_stdin", "process_id":42, "record":null});
        let Operation::Write { id, frame } = Operation::parse(write).unwrap() else {
            panic!("write operation expected");
        };
        assert_eq!(id, 42);
        assert_eq!(frame.bytes(), b"null\n");
        for args in [
            json!({"process_action":"write_stdin", "process_id":42}),
            json!({"process_action":"close_stdin", "process_id":42, "record":null}),
            json!({"stdin_profile":"jsonl_inspect", "background":true, "cmd":""}),
            json!({"stdin_profile":"jsonl_inspect", "background":true, "stdin":null}),
            json!({"stdin_profile":"jsonl_inspect", "background":true, "allow_network":false}),
            json!({"stdin_profile":"jsonl_inspect", "yield_ms":0}),
            json!({"stdin_profile":"jsonl_inspect", "yield_ms":10001}),
            json!({"stdin_profile":"jsonl_inspect"}),
            json!({"process_action":"close_stdin", "process_id":42, "redacted":true}),
        ] {
            assert!(Operation::parse(args).is_err());
        }
        assert!(matches!(
            Operation::parse(json!({"stdin_profile":"jsonl_inspect", "yield_ms":10_000})),
            Ok(Operation::Launch { .. })
        ));
    }
}
