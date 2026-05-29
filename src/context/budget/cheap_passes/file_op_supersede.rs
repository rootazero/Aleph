//! `FileOpSupersedeStage` — deterministic last-write-wins context reduction
//! for repeated file operations on the same canonical path.
//!
//! When the assistant has read, written, and re-edited the same file across
//! several turns, the earlier round-trips carry stale or redundant bytes:
//! the read bytes were superseded by a later write, and earlier writes were
//! overwritten by later ones. Their `ToolResult` bodies can safely be
//! replaced by short stubs that name the superseding tool — the assistant's
//! own `ToolCall` blocks (its reasoning intent) are preserved verbatim.
//!
//! Borrowed from claw-code `runtime/src/trident.rs` stage 1 (2026-05) with
//! the following safety upgrades vs the upstream port:
//!
//! 1. **No message deletion.** Only the `ToolResult` *content* is rewritten.
//!    The matching assistant `ToolCall` block, plus any siblings (text /
//!    thinking), are untouched so the model's reasoning chain stays intact.
//! 2. **Pressure-gated.** Below `min_pressure_ratio` the stage is a
//!    no-op — calm runs pay nothing.
//! 3. **Fresh-tail respected.** `fresh_tail_count` messages are immune.
//! 4. **Error results never touched.** A failing read followed by a
//!    successful write keeps the error text — the model may need it to
//!    explain itself.
//! 5. **Deterministic path extraction.** Only `path` / `file_path` from
//!    `ToolCall.arguments`. No output-string heuristics (claw-code's
//!    `path: ` line scan is brittle on mixed JSON / text outputs).
//!
//! R7 alignment: the rule is *structural and deterministic* (path equality
//! + op ordering). No similarity heuristic, no relevance scoring, no
//! LLM-style judgement is replicated in code.
//!
//! R10 alignment: lives in `src/context/budget/cheap_passes/`, alongside
//! the other deterministic transforms ([`ToolResultPruningStage`],
//! [`HistoricalImageStrippingStage`]). Wired into the production
//! `PreflightPipeline` in `orchestrator::harness_bridge`.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use serde_json::Value;

use crate::context::budget::preflight::PreflightStage;
use crate::context::budget::ContextPressure;
use crate::providers::message::{ContentBlock, UnifiedMessage};

/// Default fill-ratio gate (60%) below which the stage is a no-op. Matches
/// the `PressureLevel::Preventive` threshold used elsewhere in the budget
/// subsystem — at calm pressure the byte savings aren't worth the diff.
const DEFAULT_MIN_PRESSURE_RATIO: f64 = 0.60;

/// File-op last-write-wins preflight stage.
///
/// Tunable via [`FileOpSupersedeStage::new`]; sensible production defaults
/// via [`FileOpSupersedeStage::default`].
pub struct FileOpSupersedeStage {
    /// Tool names treated as "read" file ops (input: path → output: bytes).
    /// Default covers Aleph (`file_read`) plus upstream / MCP-bridged
    /// aliases (`Read`, `read_file`) so cross-system conversations pair.
    pub read_tools: Vec<String>,
    /// Tool names treated as "write" file ops (overwrite at path).
    pub write_tools: Vec<String>,
    /// Tool names treated as "edit" file ops. Edits behave like writes for
    /// supersession — a later edit replaces earlier reads / writes / edits
    /// on the same canonical path.
    pub edit_tools: Vec<String>,
    /// Minimum context fill ratio (`pressure.ratio`) before the stage fires.
    /// Below this the stage returns 0 without touching messages.
    pub min_pressure_ratio: f64,
    /// Minimum number of ops on the same path required to consider it for
    /// supersession. Default 2 (one obsolete + one current).
    pub min_ops_per_path: usize,
}

impl Default for FileOpSupersedeStage {
    fn default() -> Self {
        Self {
            read_tools: vec![
                "file_read".to_string(),
                "Read".to_string(),
                "read_file".to_string(),
            ],
            write_tools: vec![
                "file_write".to_string(),
                "Write".to_string(),
                "write_file".to_string(),
            ],
            edit_tools: vec![
                "file_edit".to_string(),
                "apply_patch".to_string(),
                "Edit".to_string(),
                "edit_file".to_string(),
            ],
            min_pressure_ratio: DEFAULT_MIN_PRESSURE_RATIO,
            min_ops_per_path: 2,
        }
    }
}

/// Classification of a file op for supersession ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileOpKind {
    Read,
    Write,
    Edit,
}

impl FileOpKind {
    /// True when this op kind invalidates earlier reads / writes / edits on
    /// the same path. Both `Write` and `Edit` are state-mutating from the
    /// LLM's perspective.
    fn is_mutating(self) -> bool {
        matches!(self, FileOpKind::Write | FileOpKind::Edit)
    }
}

/// One file op pinned to a specific `(msg_index, call_id)` coordinate.
#[derive(Debug)]
struct FileOpRef {
    msg_index: usize,
    call_id: String,
    kind: FileOpKind,
}

impl FileOpSupersedeStage {
    /// Construct with the full configuration surface explicitly set.
    /// Use [`FileOpSupersedeStage::default`] for production defaults.
    pub fn new(
        read_tools: Vec<String>,
        write_tools: Vec<String>,
        edit_tools: Vec<String>,
        min_pressure_ratio: f64,
        min_ops_per_path: usize,
    ) -> Self {
        Self {
            read_tools,
            write_tools,
            edit_tools,
            min_pressure_ratio,
            min_ops_per_path,
        }
    }

    /// Classify a tool name against the configured allowlist. Returns
    /// `None` for tools outside the file-op universe (the algorithm only
    /// reasons about file paths).
    fn classify(&self, tool_name: &str) -> Option<FileOpKind> {
        if self.read_tools.iter().any(|n| n == tool_name) {
            Some(FileOpKind::Read)
        } else if self.write_tools.iter().any(|n| n == tool_name) {
            Some(FileOpKind::Write)
        } else if self.edit_tools.iter().any(|n| n == tool_name) {
            Some(FileOpKind::Edit)
        } else {
            None
        }
    }

    /// Walk the message vector and build a `path → ops` index. Only
    /// assistant `ToolCall` blocks contribute; tool results are paired
    /// later via `tool_call_id`. The `Vec` for each path is in ascending
    /// message order because we iterate `messages` linearly.
    fn index_file_ops(&self, messages: &[UnifiedMessage]) -> BTreeMap<String, Vec<FileOpRef>> {
        let mut by_path: BTreeMap<String, Vec<FileOpRef>> = BTreeMap::new();
        for (idx, msg) in messages.iter().enumerate() {
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
                let Some(kind) = self.classify(name) else {
                    continue;
                };
                let Some(path) = canonical_path(arguments) else {
                    continue;
                };
                by_path.entry(path).or_default().push(FileOpRef {
                    msg_index: idx,
                    call_id: id.clone(),
                    kind,
                });
            }
        }
        by_path
    }

    /// Given the path → ops index, return the set of `call_id`s whose
    /// `ToolResult` body is safe to replace with a stub. The rule:
    ///
    /// 1. For each path with ≥ `min_ops_per_path` ops, find the index of
    ///    the LAST mutating op (Write or Edit). If no mutating op exists,
    ///    no op is obsolete — the model is still reading without committing.
    /// 2. Every op whose `msg_index < last_mutating_index` becomes obsolete.
    /// 3. The last mutating op is preserved — that is the canonical state.
    /// 4. Obsolete entries inside `fresh_tail_start..` are dropped — the
    ///    fresh tail is sacred.
    fn obsolete_call_ids(
        &self,
        by_path: &BTreeMap<String, Vec<FileOpRef>>,
        fresh_tail_start: usize,
    ) -> BTreeSet<String> {
        let mut obsolete: BTreeSet<String> = BTreeSet::new();
        for ops in by_path.values() {
            if ops.len() < self.min_ops_per_path {
                continue;
            }
            let Some(last_mut_idx) = ops
                .iter()
                .rev()
                .find(|op| op.kind.is_mutating())
                .map(|op| op.msg_index)
            else {
                continue;
            };
            for op in ops {
                if op.msg_index >= last_mut_idx {
                    continue;
                }
                if op.msg_index >= fresh_tail_start {
                    continue;
                }
                obsolete.insert(op.call_id.clone());
            }
        }
        obsolete
    }
}

#[async_trait]
impl PreflightStage for FileOpSupersedeStage {
    fn name(&self) -> &'static str {
        "file_op_supersede"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        if pressure.ratio < self.min_pressure_ratio {
            return 0;
        }
        let by_path = self.index_file_ops(messages);
        let fresh_tail_start = messages.len().saturating_sub(fresh_tail_count);
        let obsolete = self.obsolete_call_ids(&by_path, fresh_tail_start);
        if obsolete.is_empty() {
            return 0;
        }

        // Chars-to-tokens ratio used by the rest of the budget subsystem.
        // We avoid `estimate_tokens_smart` here because the savings figure
        // is reported to the caller for logging; staying with a fixed
        // ratio matches what `ToolResultPruningStage` does and keeps the
        // numbers comparable across stages.
        const CHARS_PER_TOKEN: f64 = 4.0;

        let mut freed_chars: usize = 0;
        let mut stubbed: usize = 0;
        for msg in messages.iter_mut() {
            let UnifiedMessage::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
            } = msg
            else {
                continue;
            };
            if !obsolete.contains(tool_call_id) {
                continue;
            }
            if *is_error {
                // Safety contract: error results carry diagnostic text the
                // LLM may rely on to plan its next move.
                continue;
            }
            let before = content_chars(content);
            *content = vec![ContentBlock::Text {
                text: stub_message(tool_name),
                cache_control: None,
            }];
            let after = content_chars(content);
            freed_chars = freed_chars.saturating_add(before.saturating_sub(after));
            stubbed = stubbed.saturating_add(1);
        }

        if stubbed > 0 {
            tracing::info!(
                target: "preflight_pipeline",
                stage = self.name(),
                stubbed,
                "stubbed superseded tool_result bodies",
            );
        }

        ((freed_chars as f64) / CHARS_PER_TOKEN).ceil() as usize
    }
}

/// Total `char` count across all text / json / thinking / tool_call
/// content blocks. Mirrors the estimator used by other compactors (Aleph
/// treats `chars / ratio` as the canonical token proxy).
fn content_chars(blocks: &[ContentBlock]) -> usize {
    blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text, .. } => text.chars().count(),
            ContentBlock::Json { value } => value.to_string().chars().count(),
            ContentBlock::Thinking { thinking, .. } => thinking.chars().count(),
            ContentBlock::ToolCall {
                name, arguments, ..
            } => name.chars().count() + arguments.to_string().chars().count(),
            ContentBlock::Image { .. } => 0,
        })
        .sum()
}

/// Extract the canonical file path from a `ToolCall.arguments` value.
/// Tries `path` first (matches `file_read`), then `file_path` (matches
/// `file_write` / `file_edit`). Returns `None` for inputs that don't
/// resolve to a string — those calls are excluded from the supersession
/// graph rather than guessed at.
fn canonical_path(args: &Value) -> Option<String> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .or_else(|| args.get("file_path").and_then(Value::as_str))?;
    Some(canonicalize_path_string(raw))
}

/// Reduce `./` prefixes and trim whitespace so two callers addressing the
/// same logical path produce the same key. Full filesystem
/// `canonicalize()` is intentionally avoided — we are running against the
/// message log, not the live FS, and the file may have been renamed since.
fn canonicalize_path_string(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("./") {
        return rest.to_string();
    }
    trimmed.to_string()
}

/// Stub text written into superseded `ToolResult` bodies. The message
/// names the originating tool so the LLM, on rare replays, can see *why*
/// the bytes are gone instead of treating the empty block as a tool
/// failure.
fn stub_message(tool_name: &str) -> String {
    format!(
        "[content superseded by a later {tool_name}/file write on the same path; \
         original output dropped during preflight compaction]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pressure(ratio: f64) -> ContextPressure {
        ContextPressure {
            used_tokens: (ratio * 10000.0) as usize,
            budget_tokens: 10000,
            ratio,
            overhead_tokens: 500,
            available_for_messages: 9500,
        }
    }

    fn read_call(id: &str, path: &str) -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: id.to_string(),
                name: "file_read".to_string(),
                arguments: json!({ "path": path }),
                thought_signature: None,
            }],
        }
    }

    fn write_call(id: &str, path: &str) -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: id.to_string(),
                name: "file_write".to_string(),
                arguments: json!({ "file_path": path, "content": "..." }),
                thought_signature: None,
            }],
        }
    }

    fn edit_call(id: &str, path: &str) -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: id.to_string(),
                name: "file_edit".to_string(),
                arguments: json!({ "file_path": path }),
                thought_signature: None,
            }],
        }
    }

    fn tool_result(id: &str, name: &str, body: &str, is_error: bool) -> UnifiedMessage {
        UnifiedMessage::ToolResult {
            tool_call_id: id.to_string(),
            tool_name: name.to_string(),
            content: vec![ContentBlock::Text {
                text: body.to_string(),
                cache_control: None,
            }],
            is_error,
        }
    }

    #[tokio::test]
    async fn read_then_write_supersedes_the_read() {
        let mut messages = vec![
            read_call("r1", "/tmp/a.txt"),
            tool_result("r1", "file_read", &"x".repeat(800), false),
            write_call("w1", "/tmp/a.txt"),
            tool_result("w1", "file_write", "wrote 12 bytes", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        assert!(freed > 0, "should report a non-zero token saving");
        let UnifiedMessage::ToolResult { content, .. } = &messages[1] else {
            panic!("index 1 must remain a ToolResult");
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert!(text.contains("superseded"));
        let UnifiedMessage::ToolResult { content, .. } = &messages[3] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert_eq!(text, "wrote 12 bytes");
    }

    #[tokio::test]
    async fn last_mutating_op_is_preserved() {
        let mut messages = vec![
            edit_call("e1", "/x.rs"),
            tool_result("e1", "file_edit", &"first diff".repeat(100), false),
            edit_call("e2", "/x.rs"),
            tool_result("e2", "file_edit", "second diff", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let _ = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        let UnifiedMessage::ToolResult { content, .. } = &messages[1] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert!(text.contains("superseded"));
        let UnifiedMessage::ToolResult { content, .. } = &messages[3] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert_eq!(text, "second diff");
    }

    #[tokio::test]
    async fn reads_only_never_supersede() {
        let mut messages = vec![
            read_call("r1", "/a"),
            tool_result("r1", "file_read", "aaa", false),
            read_call("r2", "/a"),
            tool_result("r2", "file_read", "aaa-modified", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        assert_eq!(freed, 0);
        // Both results still verbatim.
        let UnifiedMessage::ToolResult { content, .. } = &messages[1] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert_eq!(text, "aaa");
    }

    #[tokio::test]
    async fn different_paths_are_independent() {
        let mut messages = vec![
            read_call("r1", "/a"),
            tool_result("r1", "file_read", "AA", false),
            write_call("w1", "/b"),
            tool_result("w1", "file_write", "wrote b", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        assert_eq!(freed, 0);
    }

    #[tokio::test]
    async fn pressure_gate_blocks_below_threshold() {
        let mut messages = vec![
            read_call("r1", "/a"),
            tool_result("r1", "file_read", &"x".repeat(800), false),
            write_call("w1", "/a"),
            tool_result("w1", "file_write", "ok", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.40), 0).await;
        assert_eq!(freed, 0, "ratio 0.40 < 0.60 → stage must hold off");
        // Read body still verbatim.
        let UnifiedMessage::ToolResult { content, .. } = &messages[1] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert_eq!(text.chars().count(), 800);
    }

    #[tokio::test]
    async fn fresh_tail_protects_recent_messages() {
        let mut messages = vec![
            read_call("r1", "/a"),
            tool_result("r1", "file_read", "aaa", false),
            write_call("w1", "/a"),
            tool_result("w1", "file_write", "ok", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 4).await;
        assert_eq!(freed, 0, "fresh_tail_count = len ⇒ everything protected");
    }

    #[tokio::test]
    async fn error_results_are_never_stubbed() {
        let mut messages = vec![
            read_call("r1", "/a"),
            tool_result("r1", "file_read", "ENOENT: a does not exist", true),
            write_call("w1", "/a"),
            tool_result("w1", "file_write", "ok", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        // The earlier failing read IS classified as obsolete, but execute
        // refuses to rewrite an error result.
        assert_eq!(freed, 0);
        let UnifiedMessage::ToolResult {
            content, is_error, ..
        } = &messages[1]
        else {
            panic!()
        };
        assert!(*is_error);
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert_eq!(text, "ENOENT: a does not exist");
    }

    #[tokio::test]
    async fn alias_tool_names_are_classified() {
        let mut messages = vec![
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "u1".into(),
                    name: "read_file".into(),
                    arguments: json!({ "path": "/m" }),
                    thought_signature: None,
                }],
            },
            tool_result("u1", "read_file", &"y".repeat(200), false),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "u2".into(),
                    name: "Write".into(),
                    arguments: json!({ "file_path": "/m", "content": "..." }),
                    thought_signature: None,
                }],
            },
            tool_result("u2", "Write", "ok", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        assert!(freed > 0);
        let UnifiedMessage::ToolResult { content, .. } = &messages[1] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert!(text.contains("superseded"));
    }

    #[tokio::test]
    async fn dot_slash_paths_match_canonical_form() {
        let mut messages = vec![
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "r1".into(),
                    name: "file_read".into(),
                    arguments: json!({ "path": "./relative.txt" }),
                    thought_signature: None,
                }],
            },
            tool_result("r1", "file_read", &"z".repeat(500), false),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "w1".into(),
                    name: "file_write".into(),
                    arguments: json!({ "file_path": "relative.txt" }),
                    thought_signature: None,
                }],
            },
            tool_result("w1", "file_write", "ok", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        assert!(
            freed > 0,
            "`./relative.txt` and `relative.txt` must canonicalize to the same key"
        );
    }

    #[tokio::test]
    async fn no_match_when_path_argument_missing() {
        // Tool call that classifies as a file_read but lacks `path` /
        // `file_path` — we must NOT guess; the call is excluded from the
        // supersession graph entirely.
        let mut messages = vec![
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "r1".into(),
                    name: "file_read".into(),
                    arguments: json!({ "other": "x" }),
                    thought_signature: None,
                }],
            },
            tool_result("r1", "file_read", &"x".repeat(500), false),
            write_call("w1", "/a"),
            tool_result("w1", "file_write", "ok", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        assert_eq!(freed, 0);
    }

    #[test]
    fn canonical_path_strips_dot_slash_prefix() {
        assert_eq!(
            canonical_path(&json!({ "path": "./relative.txt" })).as_deref(),
            Some("relative.txt"),
        );
    }

    #[test]
    fn canonical_path_returns_none_for_non_string_path() {
        assert_eq!(canonical_path(&json!({})), None);
        assert_eq!(canonical_path(&json!({ "path": 42 })), None);
        assert_eq!(canonical_path(&json!({ "other": "x" })), None);
    }

    /// Integration: exercise `FileOpSupersedeStage` inside a real
    /// `PreflightPipeline` alongside the other two production stages
    /// (`ToolResultPruningStage`, `HistoricalImageStrippingStage`). Confirms
    /// the orchestration order chosen in `orchestrator/harness_bridge.rs`
    /// produces a clean, monotonic message vector with no stages stepping
    /// on each other's output.
    #[tokio::test]
    async fn integration_supersede_then_pruning_then_image_strip() {
        use crate::context::budget::cheap_passes::{
            HistoricalImageStrippingStage, ToolResultPruningStage,
        };
        use crate::context::budget::preflight::{PreflightPipeline, PreflightStage};

        // Conversation: opening user turn carries an EARLIER image (must be
        // stripped, it's not the newest), then read-A → write-A (read-A
        // round is superseded), read-B with large body (must be pruned),
        // then a CURRENT user turn with a newer image that must survive
        // as "live" context.
        let mut messages = vec![
            UnifiedMessage::User {
                content: vec![
                    ContentBlock::Text {
                        text: "first image".into(),
                        cache_control: None,
                    },
                    ContentBlock::Image {
                        data: "iVBORw0KGgo=".repeat(50),
                        mime_type: "image/png".into(),
                    },
                ],
            },
            read_call("r1", "/a"),
            tool_result("r1", "file_read", &"X".repeat(2000), false),
            write_call("w1", "/a"),
            tool_result("w1", "file_write", "ok", false),
            read_call("r2", "/b"),
            tool_result("r2", "file_read", &"Y".repeat(2000), false),
            UnifiedMessage::User {
                content: vec![
                    ContentBlock::Text {
                        text: "look at this newer one".into(),
                        cache_control: None,
                    },
                    ContentBlock::Image {
                        data: "iVBORw0KGgo=".repeat(50),
                        mime_type: "image/png".into(),
                    },
                ],
            },
        ];

        let pipeline = PreflightPipeline::new(vec![
            Box::new(FileOpSupersedeStage::default()) as Box<dyn PreflightStage>,
            Box::new(ToolResultPruningStage::default()),
            Box::new(HistoricalImageStrippingStage),
        ]);

        let p = pressure(0.85);
        let total_freed = pipeline.run(&mut messages, &p, 0).await;
        assert!(
            total_freed > 0,
            "the 3-stage pipeline must free a non-zero token count"
        );

        // r1's body must be the supersede stub (FileOpSupersedeStage runs
        // first, so ToolResultPruningStage sees the already-short stub and
        // chooses not to prune it further). Now at index 2.
        let UnifiedMessage::ToolResult { content, .. } = &messages[2] else {
            panic!("messages[2] must remain a ToolResult");
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!("supersede stub is single-block text");
        };
        assert!(
            text.contains("superseded"),
            "r1 body should be the supersede stub, got: {text:?}"
        );
        assert!(
            !text.starts_with("[pruned tool_result:"),
            "tool-result-pruning must not re-stub the already-tiny supersede stub",
        );

        // r2's body should have been replaced by the prune placeholder
        // because the path was never rewritten — supersede leaves it alone
        // and tool_result_pruning kicks in on the large body. Now at index 6.
        let UnifiedMessage::ToolResult { content, .. } = &messages[6] else {
            panic!("messages[6] must remain a ToolResult");
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert!(
            text.starts_with("[pruned tool_result:"),
            "r2 should be pruned (large unsuperseded body); got: {text:.60?}"
        );

        // The OPENING user-turn image (now at index 0) is historical and
        // must be stripped — there's a newer image-bearing turn at index 7.
        let UnifiedMessage::User { content } = &messages[0] else {
            panic!()
        };
        let has_image_old = content
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. }));
        assert!(
            !has_image_old,
            "historical (earlier) image must be stripped"
        );

        // The NEWEST image-bearing turn (index 7) is "live" context and
        // must survive as-is — the stripping stage preserves the newest.
        let UnifiedMessage::User { content } = &messages[7] else {
            panic!()
        };
        let has_image_new = content
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. }));
        assert!(
            has_image_new,
            "newest image-bearing turn must survive as live context"
        );

        // w1's success body is preserved — it is the canonical write
        // result, neither superseded nor over the prune threshold. Now at index 4.
        let UnifiedMessage::ToolResult { content, .. } = &messages[4] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert_eq!(text, "ok");
    }
}
