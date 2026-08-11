//! `ScratchpadProgressSink` — the R5 "AI comes to you" progress side-channel.
//!
//! `progress_parts` (`builtin_tools/scratchpad.rs`) renders the task checklist
//! into the *tool result*, which is **model-facing** only. When Aleph runs a
//! long multi-step task headless (background / daemon-triggered), the user
//! sees nothing until the final reply — a black box.
//!
//! This sink closes that gap without touching the laze loop: it is a thin
//! decorator over the inner [`TraceSink`] that recognises the scratchpad
//! progress + boundary events *already flowing* through the trace stream and
//! mirrors them to the user's active channel (Telegram / Panel / …) via the
//! existing [`ChannelRegistry`] push primitive — the same path
//! `GatewayDeliveryTarget` uses for task results.
//!
//! ## Why this is a pure I/O bypass (R4 / R10)
//!
//! * It adds **zero** new `LoopTraceEvent` variants and **zero** harness emit
//!   points — it only reads `ToolCallCompleted { scratchpad }` (carrying the
//!   `render_progress` echo) and `SessionCompleted { terminate_reason }`
//!   (carrying the watchdog boundary). No loop cognition is touched.
//! * It never blocks: `on_trace` only does a non-blocking `mpsc` send (per the
//!   `TraceSink` contract); the actual async channel push drains on a spawned
//!   task. A failed push is logged at `debug` and dropped (best-effort).
//! * It always forwards the original event to the inner sink, so trace
//!   persistence / Panel streaming are unaffected.
//!
//! Gated behind `[execution] progress_push` (default off) and only installed
//! when the run is bound to a user channel — see `run_loop.rs`.

use crate::sync_primitives::Arc;
use tokio::sync::mpsc;

use crate::gateway::channel::{ChannelId, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;

/// Translate a trace event into a human-readable progress line for the user's
/// channel, or `None` when the event carries no surfaceable progress.
///
/// Pure (no I/O) so it is trivially unit-testable — mirrors the `translate`
/// pattern in [`crate::agents::forwarding_trace_sink`].
pub(crate) fn scratchpad_progress_line(event: &LoopTraceEvent) -> Option<String> {
    use crate::orchestrator::dispatch::TerminateReason;
    use crate::tools::runtime::ToolResult;

    match event {
        // Objective set / plan laid out / step ticked — `ScratchpadOutput`
        // carries the `render_progress` checklist in `progress`.
        //
        // This used to be gated on a `PROGRESS_ACTIONS` whitelist of four
        // action names, because the echo shared the `content` field with the
        // raw markdown that `read` / `initialize` return and there was no other
        // way to tell them apart (`initialize` was once on the list, and did
        // push the whole file under a 任务进度 header). A name list only
        // describes the actions that existed the day it was written: a fifth
        // mutating action would have stopped reaching the user's channel with
        // nothing failing anywhere. The tool now says which of the two it is,
        // so the shape is the criterion and there is no list to forget.
        LoopTraceEvent::ToolCallCompleted { call, result, .. }
            if call.tool_name == "scratchpad" =>
        {
            let output = match result {
                ToolResult::Success { output } => output,
                ToolResult::Error { .. } => return None,
            };
            let content = output.get("progress")?.as_str()?.trim();
            if content.is_empty() {
                return None;
            }
            // Wrap-up: a finished goal-loop echoes a completion summary (the
            // model checked its last box). Surface it directly as the closing
            // push instead of the in-progress "Task progress" header.
            if content.starts_with(crate::memory::scratchpad::COMPLETION_BANNER) {
                return Some(content.to_string());
            }
            Some(format!("📋 任务进度\n{content}"))
        }

        // Watchdog boundary — surface the circuit-breaker so the user can intervene
        // (Dim 3a "abnormal circuit-break + proactive feedback"). The verifier keeps
        // forcing continues until this cap, so reaching it means the model could not
        // work the list down on its own.
        LoopTraceEvent::SessionCompleted {
            terminate_reason: Some(reason),
            ..
        } => match reason {
            TerminateReason::VerifierVeto { vetos } => Some(format!(
                "⚠️ 目标未达成：已连续 {vetos} 次尝试收尾但执行列表仍有未完成项，已暂停等待你的指示。"
            )),
            TerminateReason::ConsecutiveFailureCap { consecutive } => Some(format!(
                "⚠️ 任务受阻：连续 {consecutive} 轮失败，已熔断暂停，需要你介入。"
            )),
            _ => None,
        },

        _ => None,
    }
}

/// Decorator over a parent [`TraceSink`] that mirrors scratchpad progress +
/// boundary events to a user channel, while always forwarding the original
/// event to the inner sink.
pub struct ScratchpadProgressSink {
    inner: Arc<dyn TraceSink>,
    tx: mpsc::Sender<String>,
}

impl ScratchpadProgressSink {
    /// Wrap `inner`, spawning a background drain task that pushes each progress
    /// line to `channel_id` / `chat_id` via `registry`. The task ends when this
    /// sink (and its sender) drops.
    pub fn new(
        inner: Arc<dyn TraceSink>,
        registry: Arc<ChannelRegistry>,
        channel_id: String,
        chat_id: String,
    ) -> Self {
        // Bounded queue: a chatty agent loop must not grow memory without
        // bound behind a network-bound consumer. Overflow drops the progress
        // line (best-effort mirror), same as a closed receiver.
        let (tx, mut rx) = mpsc::channel::<String>(256);
        tokio::spawn(async move {
            let channel = ChannelId::new(channel_id);
            while let Some(line) = rx.recv().await {
                let message = OutboundMessage::text(chat_id.clone(), line);
                if let Err(e) = registry.send(&channel, message).await {
                    tracing::debug!(error = %e, "scratchpad progress push failed (soft)");
                }
            }
        });
        Self { inner, tx }
    }
}

impl TraceSink for ScratchpadProgressSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        if let Some(line) = scratchpad_progress_line(event) {
            // Non-blocking; drop when the queue is full (slow consumer) or the
            // receiver is closed (drain task gone) — progress is best-effort.
            let _ = self.tx.try_send(line);
        }
        self.inner.on_trace(event);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::trace::{LoopTraceSessionOutcome, ToolCallEndEvent};
    use crate::orchestrator::dispatch::TerminateReason;
    use crate::tools::runtime::ToolResult;

    fn scratchpad_completed(action: &str, progress: Option<&str>) -> LoopTraceEvent {
        let output = match progress {
            Some(c) => serde_json::json!({ "success": true, "progress": c }),
            None => serde_json::json!({ "success": true }),
        };
        LoopTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: ToolCallEndEvent {
                tool_id: "id".into(),
                tool_name: "scratchpad".into(),
                input: serde_json::json!({ "action": action, "project_id": "p" }),
                duration_ms: 5,
            },
            result: ToolResult::Success { output },
        }
    }

    #[test]
    fn complete_item_with_content_surfaces_progress() {
        let ev = scratchpad_completed("complete_item", Some("[x] A\n[~] B\n[ ] C"));
        let line = scratchpad_progress_line(&ev).expect("should surface");
        assert!(line.contains("任务进度"));
        assert!(line.contains("[~] B"));
    }

    #[test]
    fn completion_banner_surfaces_as_closing_push_not_progress() {
        // The final complete_item echoes a wrap-up summary (banner-prefixed).
        let content = format!(
            "{}：Ship auth\n全部 2 个步骤已完成：\n- [x] Design API\n- [x] Implement",
            crate::memory::scratchpad::COMPLETION_BANNER
        );
        let ev = scratchpad_completed("complete_item", Some(&content));
        let line = scratchpad_progress_line(&ev).expect("completion should surface");
        // Surfaced verbatim — NOT wrapped with the in-progress "Task progress" header.
        assert!(line.starts_with(crate::memory::scratchpad::COMPLETION_BANNER));
        assert!(!line.contains("任务进度"));
        assert!(line.contains("Ship auth"));
    }

    /// `read` returns the raw markdown in `content` and never sets `progress`,
    /// so the pull is not mistaken for a push. Before, this was true only
    /// because "read" was absent from a hand-kept action list.
    #[test]
    fn a_raw_markdown_read_is_not_surfaced_as_progress() {
        let ev = LoopTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: ToolCallEndEvent {
                tool_id: "id".into(),
                tool_name: "scratchpad".into(),
                input: serde_json::json!({ "action": "read" }),
                duration_ms: 5,
            },
            result: ToolResult::Success {
                output: serde_json::json!({
                    "success": true,
                    "content": "# Current Task\n\n## Objective\nShip\n"
                }),
            },
        };
        assert!(scratchpad_progress_line(&ev).is_none());
    }

    /// The real contract this replaced a whitelist with: whether a line is
    /// pushed is decided by the OUTPUT shape, not by the action's name. A
    /// mutating action nobody remembered to register still surfaces.
    #[test]
    fn an_unregistered_action_name_still_surfaces_when_it_carries_progress() {
        let ev = scratchpad_completed("some_future_mutation", Some("[x] A\n[ ] B"));
        let line = scratchpad_progress_line(&ev).expect("shape decides, not the name");
        assert!(line.contains("任务进度"));
    }

    #[test]
    fn mutation_without_progress_is_skipped() {
        let ev = scratchpad_completed("set_plan", None);
        assert!(scratchpad_progress_line(&ev).is_none());
    }

    #[test]
    fn non_scratchpad_tool_is_ignored() {
        let ev = LoopTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: ToolCallEndEvent {
                tool_id: "id".into(),
                tool_name: "read_file".into(),
                input: serde_json::json!({ "action": "complete_item" }),
                duration_ms: 5,
            },
            result: ToolResult::Success {
                output: serde_json::json!({ "progress": "x" }),
            },
        };
        assert!(scratchpad_progress_line(&ev).is_none());
    }

    fn session_with_reason(reason: Option<TerminateReason>) -> LoopTraceEvent {
        LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::Completed,
            iterations: 3,
            tool_calls_made: 1,
            total_tokens: 10,
            hit_limit: false,
            final_text: None,
            terminate_reason: reason,
            duration_ms: None,
            token_breakdown: None,
            tool_timeline: Vec::new(),
        }
    }

    #[test]
    fn verifier_veto_boundary_surfaces() {
        let ev = session_with_reason(Some(TerminateReason::VerifierVeto { vetos: 10 }));
        let line = scratchpad_progress_line(&ev).expect("veto should surface");
        assert!(line.contains("10"));
        assert!(line.contains("目标未达成"));
    }

    #[test]
    fn failure_cap_boundary_surfaces() {
        let ev = session_with_reason(Some(TerminateReason::ConsecutiveFailureCap {
            consecutive: 5,
        }));
        let line = scratchpad_progress_line(&ev).expect("failure cap should surface");
        assert!(line.contains("5"));
        assert!(line.contains("熔断"));
    }

    #[test]
    fn clean_completion_is_silent() {
        let ev = session_with_reason(Some(TerminateReason::Completed));
        assert!(scratchpad_progress_line(&ev).is_none());
        // No terminate_reason at all → also silent.
        assert!(scratchpad_progress_line(&session_with_reason(None)).is_none());
    }
}
