//! Structured file-change presentation attached to a file-mutating tool's
//! result — the UI side-channel, never the model-facing text.
//!
//! Computed server-side by the tool that holds the pre-image (spec R-3),
//! hoisted out of the tool's JSON before the result is flattened for the
//! model, and carried on `ToolResult.presentation` (live) and
//! `AgentTraceToolCallEnd.presentation` (replay). Hunks are a bounded view;
//! the full text is behind `trace.tool_output`.

use serde::{Deserialize, Serialize};

/// Context lines kept on each side of a change (pi `edit-diff.ts`: 4).
pub const CONTEXT_LINES: usize = 4;
/// Total hunk lines carried on the wire before the diff degrades to stats
/// only (`Unavailable::TooLarge`). Bounds the `tool_end` frame.
pub const MAX_HUNK_LINES: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineTag {
    Ctx,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HunkLine {
    pub tag: LineTag,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// 1-based first line of this hunk in the OLD file.
    pub old_start: u32,
    /// 1-based first line of this hunk in the NEW file.
    pub new_start: u32,
    pub lines: Vec<HunkLine>,
}

/// Why no hunks are attached. A closed set: a renderer shows the reason and
/// NEVER guesses (a missing pre-image is not "created").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unavailable {
    /// Either side is not valid UTF-8 text.
    Binary,
    /// The hunks exceeded [`MAX_HUNK_LINES`]; `added` / `removed` are still exact.
    TooLarge,
    /// The tool could not read the file before writing it.
    PreImageUnavailable,
    /// Text decoded but line splitting failed (e.g. lone surrogates).
    Encoding,
    /// The tool itself failed; nothing was written.
    ToolFailed,
    /// A secret pattern spanned more than one hunk line, so the hunks were
    /// dropped rather than shown. `added` / `removed` are still exact.
    ///
    /// The masker that guards this frame works per line, and the one
    /// multi-line pattern in `secret_patterns.rs` (`-----BEGIN … PRIVATE
    /// KEY----- … -----END`) needs both markers in one string — so a `.pem`
    /// written one base64 chunk per line matches nothing per line and used to
    /// ship whole. The masked replacement for that pattern *contains
    /// newlines*, so a join-mask-split would change the line count and make
    /// `old_start` / `new_start` / `added` / `removed` lie. Dropping the hunks
    /// is the honest answer: the stats are still true, and "I have the counts
    /// but cannot show you the diff" is something a renderer can say, where a
    /// silently corrupted diff is not.
    ///
    /// NOT a synonym for any of the reasons above: nothing failed, nothing was
    /// too large, and the content decoded fine. Reusing one of those would be
    /// a wrong label, which this enum's closed-set contract costs more than a
    /// missing one.
    Redacted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub kind: FileChangeKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hunks: Vec<Hunk>,
    pub added: u32,
    pub removed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<Unavailable>,
}

impl FileChange {
    /// A change whose hunks could not be produced. Stats may still be known.
    #[must_use]
    pub fn unavailable(path: impl Into<String>, kind: FileChangeKind, why: Unavailable) -> Self {
        Self {
            path: path.into(),
            kind,
            hunks: Vec::new(),
            added: 0,
            removed: 0,
            unavailable: Some(why),
        }
    }

    /// Total hunk lines (for the wire bound).
    #[must_use]
    pub fn hunk_line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }
}

/// The UI side-channel a tool result can carry. One variant today; a
/// closed enum so a second kind (e.g. a table) is a deliberate addition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Presentation {
    /// One entry per file the call touched (a single edit is a Vec of one).
    FileChanges { changes: Vec<FileChange> },
}

/// The key a tool puts this under inside its own JSON output. The dispatch
/// layer hoists and removes it BEFORE the value is flattened for the model.
pub const PRESENTATION_KEY: &str = "_presentation";

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Presentation {
        Presentation::FileChanges {
            changes: vec![FileChange {
                path: "src/a.rs".into(),
                kind: FileChangeKind::Modified,
                hunks: vec![Hunk {
                    old_start: 10,
                    new_start: 10,
                    lines: vec![
                        HunkLine { tag: LineTag::Ctx, text: "fn a() {".into() },
                        HunkLine { tag: LineTag::Del, text: "    1".into() },
                        HunkLine { tag: LineTag::Add, text: "    2".into() },
                    ],
                }],
                added: 1,
                removed: 1,
                unavailable: None,
            }],
        }
    }

    #[test]
    fn presentation_round_trips_and_is_tagged_by_kind() {
        let v = serde_json::to_value(sample()).unwrap();
        assert_eq!(v["kind"], "file_changes");
        assert_eq!(v["changes"][0]["hunks"][0]["lines"][1]["tag"], "del");
        let back: Presentation = serde_json::from_value(v).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn an_unavailable_change_carries_no_hunks_and_says_why() {
        let c = FileChange::unavailable("x.bin", FileChangeKind::Modified, Unavailable::Binary);
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("hunks").is_none(), "empty hunks are elided");
        assert_eq!(v["unavailable"], "binary");
        assert_eq!(c.hunk_line_count(), 0);
    }

    #[test]
    fn a_change_from_an_older_server_without_optional_fields_still_parses() {
        let v = serde_json::json!({
            "path": "p", "kind": "created", "added": 3, "removed": 0
        });
        let c: FileChange = serde_json::from_value(v).unwrap();
        assert!(c.hunks.is_empty());
        assert!(c.unavailable.is_none());
    }

    #[test]
    fn wire_key_names_are_locked() {
        let v = serde_json::to_value(sample()).unwrap();
        let change = &v["changes"][0];
        for k in ["path", "kind", "hunks", "added", "removed"] {
            assert!(change.get(k).is_some(), "missing wire key {k}");
        }
        for k in ["old_start", "new_start", "lines"] {
            assert!(change["hunks"][0].get(k).is_some(), "missing hunk key {k}");
        }
    }
}
