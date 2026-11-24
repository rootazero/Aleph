//! Allowlisted projection shared by live V1 chat and addressed replay.

use aleph_protocol::chat_transcript::{ChatToolChange, ChatToolDelta, CHAT_TOOL_TOPIC};
use aleph_protocol::file_change::Presentation;
use aleph_protocol::StreamEvent;
use serde_json::{json, Value};
use shared_ui_logic::transcript::{summarize, DISPLAY_NAMES};

use crate::exec::masker::{mask_presentation, SecretMasker};

pub(crate) const MAX_TOOL_FRAME_BYTES: usize = 24 * 1024;
// Leave room for the RPC envelope and the bounded run/call addresses.
const MAX_PRESENTATION_BYTES: usize = MAX_TOOL_FRAME_BYTES - 1024;

pub(crate) fn safe_tool_address(address: &str) -> bool {
    !address.is_empty()
        && address.len() <= 256
        && address
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
        && SecretMasker::new().mask_cow(address) == address
}

fn relative_display_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && path.split('/').all(|part| !part.is_empty() && part != "..")
        && path
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ' '))
}

pub(crate) fn project_tool_start(tool_name: &str, params: &Value) -> ChatToolChange {
    // The source may report a model-proposed name before registry validation.
    // Only use static labels here; never humanize unknown input for V1.
    let Some((_, label)) = DISPLAY_NAMES.iter().find(|(name, _)| *name == tool_name) else {
        return ChatToolChange::Started {
            display_name: "Tool".into(),
            args_text: String::new(),
        };
    };
    // The shared formatter's generic fallback accepts prose and commands. Only
    // feed it the one explicitly disclosable argument for these built-ins.
    let args_text = if matches!(tool_name, "file_read" | "file_edit" | "file_write") {
        params
            .get("path")
            .or_else(|| params.get("file_path"))
            .and_then(Value::as_str)
            .filter(|path| relative_display_path(path))
            .map(|path| {
                summarize(tool_name, &json!({"path": SecretMasker::new().mask(path)})).args_text
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    ChatToolChange::Started {
        display_name: (*label).into(),
        args_text,
    }
}

pub(crate) fn project_tool_finish(
    success: bool,
    duration_ms: u64,
    mut presentation: Option<Presentation>,
) -> ChatToolChange {
    let presentation_omitted = presentation.as_mut().is_some_and(|body| {
        mask_presentation(&SecretMasker::new(), body);
        serde_json::to_vec(body).map_or(true, |bytes| bytes.len() > MAX_PRESENTATION_BYTES)
    });
    ChatToolChange::Finished {
        success,
        duration_ms,
        presentation: if presentation_omitted {
            None
        } else {
            presentation
        },
        presentation_omitted,
    }
}

pub(crate) fn is_tool_source_topic(topic: &str) -> bool {
    matches!(
        topic,
        "stream.tool_start" | "stream.tool_update" | "stream.tool_end"
    )
}

/// Called only on an admitted producer notification, never on a client frame.
pub(crate) fn project_live_tool(topic: &str, payload: &Value) -> Option<String> {
    let event: StreamEvent = serde_json::from_value(payload.clone()).ok()?;
    let (run_id, seq, tool_call_id, change) = match (topic, event) {
        (
            "stream.tool_start",
            StreamEvent::ToolStart {
                run_id,
                seq,
                tool_id,
                tool_name,
                params,
            },
        ) => (
            run_id,
            seq,
            tool_id,
            project_tool_start(&tool_name, &params),
        ),
        (
            "stream.tool_update",
            StreamEvent::ToolUpdate {
                run_id,
                seq,
                tool_id,
                ..
            },
        ) => (run_id, seq, tool_id, ChatToolChange::Working),
        (
            "stream.tool_end",
            StreamEvent::ToolEnd {
                run_id,
                seq,
                tool_id,
                result,
                duration_ms,
            },
        ) => (
            run_id,
            seq,
            tool_id,
            project_tool_finish(result.success, duration_ms, result.presentation),
        ),
        _ => return None,
    };
    if !safe_tool_address(&run_id) || !safe_tool_address(&tool_call_id) {
        return None;
    }
    let params = StreamEvent::ChatTool {
        run_id,
        seq,
        delta: ChatToolDelta {
            tool_call_id,
            change,
        },
    };
    let wire = serde_json::to_string(
        &json!({"jsonrpc": "2.0", "method": CHAT_TOOL_TOPIC, "params": params}),
    )
    .ok()?;
    (wire.len() <= MAX_TOOL_FRAME_BYTES).then_some(wire)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(text: String) -> Presentation {
        serde_json::from_value(json!({
            "kind": "file_changes",
            "changes": [{"path": "src/tool.rs", "kind": "modified", "added": 1, "removed": 0,
                "hunks": [{"old_start": 1, "new_start": 1, "lines": [{"tag": "add", "text": text}]}]}]
        })).unwrap()
    }

    #[test]
    fn rich_diff_is_masked_before_bounding() {
        let secret = "AKIAIOSFODNN7EXAMPLE";
        let source = json!({
            "type": "tool_end", "run_id": "run", "tool_id": "call", "seq": 8, "duration_ms": 5,
            "result": {"success": true, "output": "raw output", "error": "raw error",
                "presentation": diff(format!("let credential = {secret};"))}
        });
        let wire = project_live_tool("stream.tool_end", &source).unwrap();
        assert!(!wire.contains(secret));
        assert!(!wire.contains("raw output"));
        assert!(!wire.contains("raw error"));
        let frame: Value = serde_json::from_str(&wire).unwrap();
        let change = &frame["params"]["change"];
        assert_eq!(change["kind"], "finished");
        assert_eq!(change["success"], true);
        assert_eq!(change["duration_ms"], 5);
        assert_eq!(change["presentation_omitted"], false);
        let line = &change["presentation"]["changes"][0]["hunks"][0]["lines"][0];
        assert_eq!(line["tag"], "add");
        assert!(line["text"]
            .as_str()
            .unwrap()
            .starts_with("let credential = "));
        let mut huge = source;
        huge["result"]["presentation"] =
            serde_json::to_value(diff("🧪".repeat(MAX_TOOL_FRAME_BYTES / 4))).unwrap();
        let wire = project_live_tool("stream.tool_end", &huge).unwrap();
        assert!(wire.len() <= MAX_TOOL_FRAME_BYTES);
        let frame: Value = serde_json::from_str(&wire).unwrap();
        let change = &frame["params"]["change"];
        assert_eq!(change["presentation_omitted"], true);
        assert!(change.get("presentation").is_none());
    }

    #[test]
    fn unregistered_proposed_name_never_reaches_the_wire() {
        let source = json!({
            "type": "tool_start", "run_id": "run", "seq": 1, "tool_id": "call",
            "tool_name": "CompanyAcquisitionDraft", "params": {"path": "unlisted-note.txt"}
        });
        let projected = project_live_tool("stream.tool_start", &source).unwrap();
        let frame: Value = serde_json::from_str(&projected).unwrap();
        assert_eq!(frame["params"]["change"]["display_name"], "Tool");
        assert!(!projected.to_lowercase().contains("acquisition"));
        assert!(!projected.contains("unlisted-note.txt"));
    }

    #[test]
    fn only_explicit_relative_file_arguments_are_disclosed() {
        assert_eq!(
            project_tool_start(
                "file_edit",
                &json!({"file_path": "src/模块.rs", "content": "private"})
            ),
            ChatToolChange::Started {
                display_name: "Edit".into(),
                args_text: "src/模块.rs".into()
            },
        );
        for forbidden in [
            "/Users/alice/private",
            "../private",
            "http://secret",
            "a?user=alice",
            "a; command",
        ] {
            assert_eq!(
                project_tool_start("file_write", &json!({"path": forbidden})),
                ChatToolChange::Started {
                    display_name: "Write".into(),
                    args_text: String::new()
                },
            );
        }
    }
}
