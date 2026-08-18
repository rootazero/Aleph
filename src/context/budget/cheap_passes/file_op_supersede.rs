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
//! 6. **Failed mutations never supersede.** A write / edit only invalidates
//!    earlier ops when its own `ToolResult` exists with `is_error == false` —
//!    a failed write left the file untouched, so the earlier read is still
//!    the model's only accurate view of it.
//! 7. **No-win and persisted-marker guards.** Mirroring
//!    [`ToolResultPruningStage`]: a body already smaller than the stub is
//!    left verbatim (stubbing it would inflate context), and
//!    `[Full output persisted: …]` markers are never stubbed — they carry
//!    the disk-recovery path the LLM needs.
//!
//! R7 alignment: the rule is *structural and deterministic* (path equality
//!   + op ordering). No similarity heuristic, no relevance scoring, no
//!     LLM-style judgement is replicated in code.
//!
//! R10 alignment: lives in `src/context/budget/cheap_passes/`, alongside
//! the other deterministic transforms ([`ToolResultPruningStage`],
//! [`HistoricalImageStrippingStage`]). Wired into the production
//! `PreflightPipeline` in `orchestrator::harness_bridge`.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;

use crate::context::budget::preflight::PreflightStage;
use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::budget::ContextPressure;
use crate::context::file_ops::{self, FileOp};
use crate::providers::message::{ContentBlock, UnifiedMessage};

/// Default fill-ratio gate (60%) below which the stage is a no-op — the
/// "preventive" band of the budget subsystem. At calm pressure the byte
/// savings aren't worth the diff, so the preflight pipeline must hand this
/// stage the *real* `ContextPressure` (see `ContextBudget::peek_pressure`),
/// not a placeholder, for the gate to mean anything.
const DEFAULT_MIN_PRESSURE_RATIO: f64 = 0.60;

/// File-op last-write-wins preflight stage.
///
/// Sensible production defaults via [`FileOpSupersedeStage::default`]; tune
/// the pressure gate via [`FileOpSupersedeStage::with_min_pressure_ratio`].
pub struct FileOpSupersedeStage {
    /// Minimum context fill ratio (`pressure.ratio`) before the stage fires.
    /// Below this the stage returns 0 without touching messages.
    pub min_pressure_ratio: f64,
}

impl Default for FileOpSupersedeStage {
    fn default() -> Self {
        Self {
            min_pressure_ratio: DEFAULT_MIN_PRESSURE_RATIO,
        }
    }
}

/// Minimum number of ops on the same path required to consider it for
/// supersession (one obsolete + one current).
///
/// A `pub` field until the compaction file ledger arrived and made the tool
/// tables a shared concern: nothing outside this file had ever written it, or
/// the three tool-name `Vec`s beside it, so all four were withdrawn (R10). The
/// tables now live in [`crate::context::file_ops`], where the ledger reads the
/// same ones — a second copy is exactly the drift this consolidation prevents.
const MIN_OPS_PER_PATH: usize = 2;

impl FileOpSupersedeStage {
    /// Override just the pressure gate. Production wires this from
    /// [`ContextBudgetConfig::preventive_floor`](crate::context::budget::ContextBudgetConfig::preventive_floor)
    /// so all three cheap passes share one config-derived band instead of this
    /// stage carrying its own hardcoded ratio.
    #[must_use]
    pub fn with_min_pressure_ratio(mut self, ratio: f64) -> Self {
        self.min_pressure_ratio = ratio;
        self
    }

    /// Group the window's file ops by canonical path, ascending message order
    /// within each path.
    ///
    /// The scan itself is [`file_ops::index_file_ops`] — shared with the
    /// compaction file ledger so "which calls are file ops, and what path do
    /// they name" has exactly one answer in the repo. Only the grouping is
    /// local, because only supersession needs it.
    fn ops_by_path(messages: &[UnifiedMessage]) -> BTreeMap<String, Vec<FileOp>> {
        let mut by_path: BTreeMap<String, Vec<FileOp>> = BTreeMap::new();
        for op in file_ops::index_file_ops(messages) {
            by_path.entry(op.path.clone()).or_default().push(op);
        }
        by_path
    }

    /// Given the path → ops index, return a `call_id → superseding tool name`
    /// map for every op whose `ToolResult` body is safe to replace with a
    /// stub. The rule:
    ///
    /// 1. For each path with ≥ `min_ops_per_path` ops, find the LAST
    ///    *successful* mutating op (Write or Edit whose paired `ToolResult`
    ///    is in `successful`). If none exists, no op is obsolete — either
    ///    the model is still reading without committing, or every mutation
    ///    failed and the earlier reads are still the accurate view.
    /// 2. Every op whose `msg_index < last_mutating_index` becomes obsolete,
    ///    recorded against the superseder's tool name (quoted in the stub).
    /// 3. The last successful mutating op is preserved — that is the
    ///    canonical state.
    /// 4. Obsolete entries inside `fresh_tail_start..` are dropped — the
    ///    fresh tail is sacred.
    fn obsolete_call_ids(
        by_path: &BTreeMap<String, Vec<FileOp>>,
        successful: &BTreeSet<String>,
        fresh_tail_start: usize,
    ) -> BTreeMap<String, String> {
        let mut obsolete: BTreeMap<String, String> = BTreeMap::new();
        for ops in by_path.values() {
            if ops.len() < MIN_OPS_PER_PATH {
                continue;
            }
            let Some(last_mut) = ops
                .iter()
                .rev()
                .find(|op| op.kind.is_mutating() && successful.contains(&op.call_id))
            else {
                continue;
            };
            for op in ops {
                if op.msg_index >= last_mut.msg_index {
                    continue;
                }
                if op.msg_index >= fresh_tail_start {
                    continue;
                }
                obsolete.insert(op.call_id.clone(), last_mut.tool_name.clone());
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
        let by_path = Self::ops_by_path(messages);
        let fresh_tail_start = messages.len().saturating_sub(fresh_tail_count);
        let successful = file_ops::successful_result_ids(messages);
        let obsolete = Self::obsolete_call_ids(&by_path, &successful, fresh_tail_start);
        if obsolete.is_empty() {
            return 0;
        }

        let mut freed_tokens: usize = 0;
        let mut stubbed: usize = 0;
        // Bound the rewrite to messages before the fresh tail: a ToolResult
        // sits one index after its ToolCall, so a call just below the boundary
        // can have its result *inside* the protected tail — the obsolete set
        // only checks the call's index, so the tail must be re-guarded here
        // (safety contract 3: fresh-tail messages are immune).
        for msg in messages.iter_mut().take(fresh_tail_start) {
            let UnifiedMessage::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } = msg
            else {
                continue;
            };
            let Some(superseder_tool) = obsolete.get(tool_call_id.as_str()) else {
                continue;
            };
            if *is_error {
                // Safety contract: error results carry diagnostic text the
                // LLM may rely on to plan its next move.
                continue;
            }
            let original_text = joined_text(content);
            // Already-persisted markers (Layer 2 of the tool-result budget)
            // are compact and carry the disk path the LLM needs to recover
            // the full output — mirror `ToolResultPruningStage`'s guard.
            if original_text.starts_with("[Full output persisted: ") {
                continue;
            }
            let replacement = stub_message(superseder_tool);
            // Freed tokens use `estimate_tokens_smart` like the sibling
            // `ToolResultPruningStage`, so the per-stage savings reported to
            // the caller are comparable across stages.
            let original_tokens = estimate_tokens_smart(&original_text);
            let new_tokens = estimate_tokens_smart(&replacement);
            // No-win guard: a body already at or below the stub's size must
            // stay verbatim — stubbing it would inflate the context. This
            // also makes repeated passes byte-stable: an already-stubbed
            // result is never rewritten again.
            if new_tokens >= original_tokens {
                continue;
            }
            *content = vec![ContentBlock::Text {
                text: replacement,
                cache_control: None,
            }];
            freed_tokens = freed_tokens.saturating_add(original_tokens - new_tokens);
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

        freed_tokens
    }
}

/// Join the text-bearing blocks of a tool-result body (Text + serialized
/// Json) exactly like `UnifiedMessage::tool_result_info`, so the
/// persisted-marker and token-accounting guards see the same bytes as the
/// sibling `ToolResultPruningStage`. Image blocks contribute nothing — an
/// image-only result therefore never clears the no-win guard and is left
/// intact for `HistoricalImageStrippingStage` to police.
fn joined_text(blocks: &[ContentBlock]) -> String {
    let mut result = String::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => {
                if !result.is_empty() {
                    result.push(' ');
                }
                result.push_str(text);
            }
            ContentBlock::Json { value } => {
                if !result.is_empty() {
                    result.push(' ');
                }
                result.push_str(&value.to_string());
            }
            _ => {}
        }
    }
    result
}

/// Extract the canonical file path from a `ToolCall.arguments` value.
/// Stub text written into superseded `ToolResult` bodies. The message
/// names the SUPERSEDING tool so the LLM, on rare replays, can see *which
/// later operation* made these bytes stale instead of treating the empty
/// block as a tool failure. The text is a pure function of the superseder's
/// tool name, so repeated passes over the same history are byte-identical
/// (prompt-cache friendly).
fn stub_message(superseder_tool: &str) -> String {
    format!(
        "[content superseded by a later {superseder_tool} on the same path; \
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
    async fn failed_write_does_not_supersede_prior_read() {
        // The write's ToolResult came back is_error = true: the file on disk
        // still matches the earlier read, which must stay verbatim — stubbing
        // it would falsely claim "superseded by a later operation".
        let mut messages = vec![
            read_call("r1", "/tmp/a.txt"),
            tool_result("r1", "file_read", &"x".repeat(800), false),
            write_call("w1", "/tmp/a.txt"),
            tool_result("w1", "file_write", "EACCES: permission denied", true),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        assert_eq!(freed, 0, "a failed mutation must not supersede anything");
        let UnifiedMessage::ToolResult { content, .. } = &messages[1] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert_eq!(text.chars().count(), 800, "read body must stay verbatim");
    }

    #[tokio::test]
    async fn persisted_marker_results_are_never_stubbed() {
        // A persisted marker longer than the stub (so only the marker guard —
        // not the no-win guard — protects it) must keep its disk-recovery path.
        let marker = format!(
            "[Full output persisted: /tmp/aleph/{}.txt (12000 tokens, file_read)]",
            "a".repeat(300)
        );
        let mut messages = vec![
            read_call("r1", "/tmp/a.txt"),
            tool_result("r1", "file_read", &marker, false),
            write_call("w1", "/tmp/a.txt"),
            tool_result("w1", "file_write", "ok", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        assert_eq!(freed, 0, "persisted markers must never be stubbed");
        let UnifiedMessage::ToolResult { content, .. } = &messages[1] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert_eq!(text, &marker, "marker text must remain verbatim");
    }

    #[tokio::test]
    async fn stub_names_the_superseding_tool() {
        let mut messages = vec![
            read_call("r1", "/tmp/a.txt"),
            tool_result("r1", "file_read", &"x".repeat(800), false),
            edit_call("e1", "/tmp/a.txt"),
            tool_result("e1", "file_edit", "ok", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let _ = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        let UnifiedMessage::ToolResult { content, .. } = &messages[1] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert!(
            text.contains("file_edit"),
            "stub must name the superseding op, not the stubbed one; got: {text}"
        );
    }

    #[tokio::test]
    async fn stub_never_replaces_a_smaller_body() {
        // No-win guard: the read body is already smaller than the stub text,
        // so rewriting it would inflate the context.
        let mut messages = vec![
            read_call("r1", "/tmp/a.txt"),
            tool_result("r1", "file_read", "tiny", false),
            write_call("w1", "/tmp/a.txt"),
            tool_result("w1", "file_write", "ok", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        assert_eq!(freed, 0);
        let UnifiedMessage::ToolResult { content, .. } = &messages[1] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert_eq!(text, "tiny", "a body smaller than the stub stays verbatim");
    }

    #[tokio::test]
    async fn freed_accounting_uses_smart_estimator() {
        let body = "x".repeat(800);
        let mut messages = vec![
            read_call("r1", "/tmp/a.txt"),
            tool_result("r1", "file_read", &body, false),
            write_call("w1", "/tmp/a.txt"),
            tool_result("w1", "file_write", "wrote 12 bytes", false),
        ];
        let stage = FileOpSupersedeStage::default();
        let freed = stage.prepare(&mut messages, &pressure(0.75), 0).await;
        let expected =
            estimate_tokens_smart(&body) - estimate_tokens_smart(&stub_message("file_write"));
        assert_eq!(
            freed, expected,
            "freed must use the same estimator as ToolResultPruningStage"
        );
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
            file_ops::canonical_path(&json!({ "path": "./relative.txt" })).as_deref(),
            Some("relative.txt"),
        );
    }

    #[test]
    fn canonical_path_returns_none_for_non_string_path() {
        assert_eq!(file_ops::canonical_path(&json!({})), None);
        assert_eq!(file_ops::canonical_path(&json!({ "path": 42 })), None);
        assert_eq!(file_ops::canonical_path(&json!({ "other": "x" })), None);
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
