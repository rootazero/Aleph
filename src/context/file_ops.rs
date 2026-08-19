//! Shared, deterministic reading of file-operation tool calls out of a message
//! window.
//!
//! Two subsystems ask the same question of the same bytes and must never learn
//! two different answers to it:
//!
//! * [`crate::context::budget::cheap_passes::file_op_supersede`] — "which
//!   earlier result did a later write make stale?"
//! * [`crate::context::compact::file_carry`] — "which files has this
//!   conversation actually read and modified?"
//!
//! The classification tables, the path key, and the "did this call succeed?"
//! predicate live here so a new tool alias (or a second argument spelling) is
//! one edit, not two. This module is pure: no I/O, no filesystem, no config.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::providers::message::{ContentBlock, UnifiedMessage};

/// Tool names treated as *read* file ops (input: path → output: bytes).
/// Covers Aleph's own `file_read` plus the upstream / MCP-bridged aliases a
/// cross-system conversation can carry.
pub(crate) const READ_TOOLS: &[&str] = &["file_read", "Read", "read_file"];

/// Tool names treated as *write* (whole-file overwrite) file ops.
pub(crate) const WRITE_TOOLS: &[&str] = &["file_write", "Write", "write_file"];

/// Tool names treated as *edit* (in-place patch) file ops. Edits behave like
/// writes for both consumers — a later edit supersedes earlier ops, and an
/// edited path counts as modified.
pub(crate) const EDIT_TOOLS: &[&str] = &["file_edit", "apply_patch", "Edit", "edit_file"];

/// Classification of a file op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileOpKind {
    Read,
    Write,
    Edit,
}

impl FileOpKind {
    /// True when this op kind changes the file on disk. Both `Write` and `Edit`
    /// are state-mutating from the model's perspective.
    pub(crate) const fn is_mutating(self) -> bool {
        matches!(self, Self::Write | Self::Edit)
    }
}

/// Classify `tool_name` against the shared tables. `None` for tools outside the
/// file-op universe — neither consumer guesses at those.
#[must_use]
pub(crate) fn classify(tool_name: &str) -> Option<FileOpKind> {
    if READ_TOOLS.contains(&tool_name) {
        Some(FileOpKind::Read)
    } else if WRITE_TOOLS.contains(&tool_name) {
        Some(FileOpKind::Write)
    } else if EDIT_TOOLS.contains(&tool_name) {
        Some(FileOpKind::Edit)
    } else {
        None
    }
}

/// The canonical path key for a file-op call's arguments.
///
/// Tries `path` first (matches `file_read`), then `file_path` (matches
/// `file_write` / `file_edit`). Returns `None` for inputs that don't resolve to
/// a string — those calls are excluded rather than guessed at.
#[must_use]
pub(crate) fn canonical_path(args: &Value) -> Option<String> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .or_else(|| args.get("file_path").and_then(Value::as_str))?;
    Some(canonicalize_path_string(raw))
}

/// Reduce `./` prefixes and trim whitespace so two callers addressing the same
/// logical path produce the same key. Full filesystem `canonicalize()` is
/// intentionally avoided — this runs against the message log, not the live FS,
/// and the file may have been renamed or deleted since.
#[must_use]
pub(crate) fn canonicalize_path_string(raw: &str) -> String {
    let trimmed = raw.trim();
    trimmed.strip_prefix("./").unwrap_or(trimmed).to_string()
}

/// The `tool_call_id`s in `messages` whose `ToolResult` arrived with
/// `is_error == false`.
///
/// Both consumers gate on this and for the same reason: a call that failed did
/// not change the world. A failed write left the file untouched (so an earlier
/// read is still the accurate view, and the ledger must not claim the file was
/// modified); a failed read produced no bytes (so the ledger must not tell the
/// model it already has the content).
#[must_use]
pub(crate) fn successful_result_ids(messages: &[UnifiedMessage]) -> BTreeSet<String> {
    messages
        .iter()
        .filter_map(|msg| match msg {
            UnifiedMessage::ToolResult {
                tool_call_id,
                is_error: false,
                ..
            } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect()
}

/// One classified file op, pinned to the message it was issued from.
#[derive(Debug, Clone)]
pub(crate) struct FileOp {
    pub(crate) msg_index: usize,
    pub(crate) call_id: String,
    pub(crate) kind: FileOpKind,
    /// Tool name from the `ToolCall` block, kept so a consumer can quote the
    /// operation that superseded another one.
    pub(crate) tool_name: String,
    pub(crate) path: String,
}

/// Every classified file op in `messages`, in ascending message order.
///
/// Only assistant `ToolCall` blocks contribute — results are paired later by
/// `tool_call_id`, which is what lets both consumers reason about success
/// without re-parsing tool output.
#[must_use]
pub(crate) fn index_file_ops(messages: &[UnifiedMessage]) -> Vec<FileOp> {
    let mut ops = Vec::new();
    for (msg_index, msg) in messages.iter().enumerate() {
        let UnifiedMessage::Assistant { content } = msg else {
            continue;
        };
        for block in content {
            let ContentBlock::ToolCall {
                id,
                name,
                arguments,
                ..
            } = block
            else {
                continue;
            };
            let (Some(kind), Some(path)) = (classify(name), canonical_path(arguments)) else {
                continue;
            };
            ops.push(FileOp {
                msg_index,
                call_id: id.clone(),
                kind,
                tool_name: name.clone(),
                path,
            });
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, name: &str, args: Value) -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: id.into(),
                name: name.into(),
                arguments: args,
                thought_signature: None,
            }],
        }
    }

    #[test]
    fn the_three_tables_are_disjoint() {
        // A name in two tables would make `classify` order-dependent, and the
        // two consumers read the classification for opposite purposes
        // (supersede *by* mutation vs report *as* modified).
        let all: Vec<&&str> = READ_TOOLS
            .iter()
            .chain(WRITE_TOOLS)
            .chain(EDIT_TOOLS)
            .collect();
        let unique: BTreeSet<&str> = all.iter().map(|n| **n).collect();
        assert_eq!(all.len(), unique.len(), "tool tables must be disjoint");
    }

    #[test]
    fn classifies_each_table_and_rejects_outsiders() {
        assert_eq!(classify("file_read"), Some(FileOpKind::Read));
        assert_eq!(classify("file_write"), Some(FileOpKind::Write));
        assert_eq!(classify("apply_patch"), Some(FileOpKind::Edit));
        assert_eq!(classify("bash"), None);
        assert!(FileOpKind::Write.is_mutating() && FileOpKind::Edit.is_mutating());
        assert!(!FileOpKind::Read.is_mutating());
    }

    #[test]
    fn path_and_file_path_spellings_produce_the_same_key() {
        assert_eq!(
            canonical_path(&json!({"path": "./src/a.rs"})),
            canonical_path(&json!({"file_path": "src/a.rs"}))
        );
        assert_eq!(canonical_path(&json!({"path": 7})), None);
        assert_eq!(canonical_path(&json!({})), None);
    }

    #[test]
    fn indexes_calls_in_order_and_skips_unclassifiable_ones() {
        let msgs = vec![
            call("c1", "file_read", json!({"path": "a.rs"})),
            call("c2", "bash", json!({"command": "ls"})),
            call("c3", "file_edit", json!({"file_path": "./a.rs"})),
        ];
        let ops = index_file_ops(&msgs);
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].call_id, "c1");
        assert_eq!(ops[1].kind, FileOpKind::Edit);
        assert_eq!(ops[0].path, ops[1].path, "./ prefix is reduced");
        assert!(ops[0].msg_index < ops[1].msg_index);
    }

    #[test]
    fn only_non_error_results_count_as_successful() {
        let msgs = vec![
            UnifiedMessage::tool_result("c1", "file_write", "ok", false),
            UnifiedMessage::tool_result("c2", "file_write", "boom", true),
        ];
        let ok = successful_result_ids(&msgs);
        assert!(ok.contains("c1"));
        assert!(!ok.contains("c2"));
    }
}
