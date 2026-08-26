//! Maps Gateway streaming events (run.*) to `ChatState` mutations.

// The peer-echo predicate lives in `aleph-protocol`, beside the frame it
// judges: the TUI has to answer the same question and cannot see this crate,
// and two copies of a delivery predicate is two answers — the wrong one
// renders a user their own message twice.
use super::state::{
    ChatState, ContextUsage, ModelInfo, ProviderRetryNotice, RunCost, ToolSettlement,
};
use crate::context::{DashboardState, GatewayEvent};
use crate::i18n::{td_string, I18nCtx, Locale};
use crate::state::layout::WorkspaceState;
use crate::state::notifications::{AskOptionView, AskQuestionView, PendingAskView};
use crate::state::sessions::SessionMap;
use crate::state::user_directory::UserDirectoryState;
use aleph_protocol::peer_message_is_renderable;
use leptos::prelude::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Parse one entry of the `AskUser` frame's structured `questions` array.
///
/// A question with no `prompt` is dropped rather than rendered blank: an empty
/// card is indistinguishable from a bug, and the flat `question` field is still
/// there to fall back on.
fn parse_ask_question(value: &serde_json::Value) -> Option<AskQuestionView> {
    let prompt = value.get("prompt")?.as_str()?.to_string();
    Some(AskQuestionView {
        id: value
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        header: value
            .get("header")
            .and_then(|v| v.as_str())
            .filter(|h| !h.trim().is_empty())
            .map(String::from),
        prompt,
        options: value
            .get("options")
            .and_then(serde_json::Value::as_array)
            .map(|opts| {
                opts.iter()
                    .filter_map(|o| {
                        Some(AskOptionView {
                            label: o.get("label")?.as_str()?.to_string(),
                            description: o
                                .get("description")
                                .and_then(|d| d.as_str())
                                .filter(|d| !d.trim().is_empty())
                                .map(String::from),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        multi_select: value
            .get("multi_select")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        secret: value
            .get("secret")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

/// Project one persisted/live `AgentTraceEvent` (tagged `kind` or `type`)
/// onto `ChatState` (step bubbles, tool status, narration) + `WorkspaceState`
/// (tool args/result payloads, current iteration). The single projection
/// shared by the live WS stream and `trace.by_runs` replay so the two paths
/// can never drift.
pub(crate) fn apply_trace_event(
    chat: ChatState,
    workspace: WorkspaceState,
    run_id: &str,
    trace_event: &serde_json::Value,
    locale: Locale,
) {
    // The harness serializes `LoopTraceEvent` with `#[serde(tag =
    // "type")]`, so the discriminator arrives as `type`. The
    // protocol `AgentTraceEvent` form tags it `kind`; accept either
    // so both wire shapes parse.
    let kind = trace_event
        .get("type")
        .or_else(|| trace_event.get("kind"))
        .and_then(|k| k.as_str())
        .unwrap_or("");

    match kind {
        "tool_call_started" => {
            let call = trace_event.get("call");
            let tool_id = call
                .and_then(|c| c.get("tool_id"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let tool_name = call
                .and_then(|c| c.get("tool_name"))
                .and_then(|t| t.as_str())
                .unwrap_or("tool");
            chat.update_tool(run_id, tool_id, tool_name, "running", None);
            // No badge bump here. A tool starting is not something the right
            // pane shows — it shows what the session *produced* — and the
            // badge that used to fire from this line was inspector-era
            // residue: it advertised a pane whose contents had not changed,
            // and was silent when they had. `ArtifactsSurface` owns it now,
            // driven by the artifact listing.
            //
            // Capture args/input for the chat column's tool card. Schema
            // varies by tool kind — try the two known keys.
            if !tool_id.is_empty() {
                let args = call
                    .and_then(|c| c.get("input").or_else(|| c.get("args")))
                    .cloned();
                if let Some(args) = args {
                    workspace.record_tool_args(run_id, tool_id, args);
                }
            }
        }
        "tool_call_completed" => {
            let call = trace_event.get("call");
            let tool_id = call
                .and_then(|c| c.get("tool_id"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let tool_name = call
                .and_then(|c| c.get("tool_name"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let duration = call
                .and_then(|c| c.get("duration_ms"))
                .and_then(serde_json::Value::as_u64);
            let result = trace_event
                .get("result")
                .unwrap_or(&serde_json::Value::Null);
            let status = if result.get("Error").is_some() {
                "failed"
            } else {
                "completed"
            };
            chat.update_tool(run_id, tool_id, tool_name, status, duration);
            // Mirror the result into workspace state so the
            // tool-detail view can show actual output.
            if !tool_id.is_empty() {
                workspace.record_tool_result(run_id, tool_id, result.clone());
            }
            // Project scratchpad plan snapshots into the sticky Todo panel.
            if tool_name == "scratchpad" {
                let action = call
                    .and_then(|c| c.get("input"))
                    .and_then(|i| i.get("action"))
                    .and_then(|a| a.as_str())
                    .unwrap_or("");
                // The harness serializes a tool's structured output as a
                // JSON-encoded STRING inside `Success.output` (the same text the
                // model is shown), so `output` is a `Value::String`, not an
                // object — `o.get("snapshot")` on a string is always `None`.
                // Decode the string first; tolerate the already-object form too.
                let output = result.get("Success").and_then(|s| s.get("output"));
                let decoded = output
                    .and_then(serde_json::Value::as_str)
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
                let snapshot = decoded.as_ref().or(output).and_then(|o| o.get("snapshot"));
                // Sink the prior plan before the new snapshot overwrites it.
                // Only a fresh decomposition (`set_plan`) or explicit teardown
                // (`clear`) supersedes — `start_item`/`complete_item`/
                // `set_objective` are in-place updates to the SAME plan and must
                // not archive. Gated on `has_activity` so a pristine refinement
                // is silently replaced.
                if action == "set_plan" || action == "clear" {
                    chat.archive_active_plan(super::state::ArchiveGate::Activity);
                }
                chat.apply_plan_update(super::plan::scratchpad_plan_update(action, snapshot));
            }
        }
        "tool_summary" => {
            if let Some(summary) = trace_event.get("summary").and_then(|s| s.as_str()) {
                append_reasoning(chat, summary);
            }
        }
        "turn_started" => {
            let Some(iteration) = trace_event
                .get("iteration")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as usize)
            else {
                return;
            };
            chat.begin_step(run_id, iteration);
            // Turn boundary reached with prompts still queued → ask the composer
            // (which owns the send pipeline) to steer them into the live run.
            // Guarded so we only wake the flush Effect when there's something to
            // flush.
            if chat.active_run_id.get_untracked().is_some()
                && !chat.prompt_queue.get_untracked().is_empty()
            {
                chat.flush_pulse.update(|n| *n = n.wrapping_add(1));
            }
        }
        "text_emitted" => {
            let Some(iteration) = trace_event
                .get("iteration")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as usize)
            else {
                return;
            };
            let text = trace_event
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("");
            chat.set_step_text(run_id, iteration, text);
        }
        // Recovery: context overflowed → history compacted → retried. Surface
        // the "what problem / how handled / outcome" so a long run isn't a
        // silent pause.
        "reactive_compaction_attempted" => {
            let succeeded = trace_event
                .get("succeeded")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let note = if succeeded {
                td_string!(locale, narration.compaction_ok).to_string()
            } else {
                td_string!(locale, narration.compaction_failed).to_string()
            };
            append_reasoning(chat, &note);
        }
        // Watchdog: the model tried to finish but its scratchpad checklist
        // still has unchecked items, so the loop was forced to continue.
        // Surface the interception reason (R5 — no black box).
        "verifier_veto" => {
            let reason = trace_event
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            append_reasoning(
                chat,
                &format!(
                    "🔁 {}: {reason}",
                    td_string!(locale, narration.verifier_veto)
                ),
            );
        }
        // MoA (Mixture-of-Agents) advisor fan-out — one event per advisor per
        // consultation. Rendered as a reasoning block so it appears inline
        // with tool_summary/verifier_veto narration (same sink, live +
        // replay both covered).
        "moa_advisor" => {
            let index = trace_event
                .get("index")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let count = trace_event
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let label = trace_event
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let text = trace_event
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let error = trace_event.get("error").and_then(|v| v.as_str());
            let advisor = td_string!(locale, narration.moa_advisor);
            if count == 0 {
                // Activation failure (runner build error): MoA didn't engage.
                append_reasoning(
                    chat,
                    &format!(
                        "⚠ {}: {}",
                        td_string!(locale, narration.moa_inactive),
                        error.unwrap_or("unknown")
                    ),
                );
            } else if let Some(err) = error {
                append_reasoning(
                    chat,
                    &format!("◇ {advisor} {index}/{count} — {label}\n⚠ {err}"),
                );
            } else {
                append_reasoning(
                    chat,
                    &format!("◇ {advisor} {index}/{count} — {label}\n{text}"),
                );
            }
        }
        // Fan-out complete; the aggregator (acting model) is being called.
        "moa_aggregating" => {
            let aggregator = trace_event
                .get("aggregator")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let n = trace_event
                .get("advisor_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let cached = trace_event
                .get("cached")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let aggregating = td_string!(locale, narration.moa_aggregating);
            if cached {
                append_reasoning(
                    chat,
                    &format!(
                        "◆ {aggregating} ({aggregator}, {})",
                        td_string!(locale, narration.moa_cached_advisors)
                    ),
                );
            } else {
                append_reasoning(
                    chat,
                    &format!(
                        "◆ {aggregating} ({aggregator}, {n} {})",
                        td_string!(locale, narration.moa_advisors)
                    ),
                );
            }
        }
        // Summed advisor spend for one fan-out (priced separately from the
        // aggregator's own usage — see LoopTraceEvent::MoaAdvisorSpend).
        "moa_advisor_spend" => {
            let input = trace_event
                .get("input_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let output = trace_event
                .get("output_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let billed = trace_event
                .get("billed_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let n = trace_event
                .get("advisor_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let cost = trace_event
                .get("cost_usd")
                .and_then(serde_json::Value::as_f64);
            let cost_str = cost.map_or(String::new(), |c| format!(" ≈ ${c:.4}"));
            append_reasoning(
                chat,
                &format!(
                    "▫ {}: {input}+{output} tokens ({billed}/{n} {}){cost_str}",
                    td_string!(locale, narration.moa_spend),
                    td_string!(locale, narration.moa_billed)
                ),
            );
        }
        // Heavy audit record — arrives only via trace.by_runs REPLAY (never
        // wire-whitelisted). Renders the full "why did MoA advise this" view
        // into the reasoning panel (round-2 W3b).
        "moa_turn_trace" => {
            let preset = trace_event
                .get("preset")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let payload = trace_event.get("payload").cloned().unwrap_or_default();
            let mut block = format!(
                "📋 {} {preset}",
                td_string!(locale, narration.moa_turn_trace_preset)
            );
            if let Some(advisors) = payload
                .get("advisors")
                .and_then(serde_json::Value::as_array)
            {
                let advisor = td_string!(locale, narration.moa_advisor);
                for (i, a) in advisors.iter().enumerate() {
                    let label = a.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    let output = a.get("output").and_then(|v| v.as_str()).unwrap_or("");
                    block.push_str(&format!(
                        "\n─── {advisor} {} — {label} ───\n{output}",
                        i + 1
                    ));
                }
            }
            if let Some(out) = payload.get("aggregator_output").and_then(|v| v.as_str()) {
                let status = payload
                    .get("aggregator_status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ok");
                block.push_str(&format!(
                    "\n─── {} ({status}) ───\n{out}",
                    td_string!(locale, narration.moa_aggregator)
                ));
            }
            append_reasoning(chat, &block);
        }
        // Live per-call cache telemetry → the composer's `cache N%` chip.
        // Fires once per LLM call (whitelisted onto the wire by
        // `AgentTraceEmitSink` for exactly this purpose). The percentage is
        // the canonical `read / (input + read)` — the same number the TUI
        // status bar and the core DB rollup show for the same call.
        "provider_usage" => {
            if let Some(pct) = provider_usage_pct(trace_event) {
                chat.live_cache_pct.set(Some(pct));
            }
        }
        _ => {}
    }
}

/// The `cache N%` chip value from one live `provider_usage` trace event, or
/// `None` when the call reported no cache activity (a cache-less provider
/// must never surface a misleading 0%). The formula is the canonical
/// `aleph_protocol::cache_hit_ratio` — `read / (input + read)`, the same
/// number the TUI status bar and the core DB rollup show for the same call.
/// Pure so the wire-shape contract is host-testable.
fn provider_usage_pct(trace_event: &serde_json::Value) -> Option<u64> {
    let read = trace_event
        .get("cache_read_tokens")
        .and_then(serde_json::Value::as_u64);
    let creation = trace_event
        .get("cache_creation_tokens")
        .and_then(serde_json::Value::as_u64);
    if read.unwrap_or(0) == 0 && creation.unwrap_or(0) == 0 {
        return None;
    }
    let input = trace_event
        .get("input_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    // `unwrap_or(0.0)`: a pure cold write (creation reported, read not) is a
    // 0% call, not an unknown one.
    let ratio = aleph_protocol::cache_hit_ratio(input, read).unwrap_or(0.0);
    Some((ratio * 100.0).round() as u64)
}

/// Reconstruct one assistant run's chat bubbles + workspace tool payloads from
/// its persisted trace `events`, mirroring the live event order: open the
/// `assistant-{run}` placeholder, project every event through
/// `apply_trace_event`, finalize the turn, then overwrite the trailing answer
/// bubble's text with the history-authoritative `final_content`. Earlier turns
/// become `intermediate-{run}-{n}` bubbles; the trailing `assistant-{run}`
/// bubble is the final answer.
pub(crate) fn replay_run(
    chat: ChatState,
    workspace: WorkspaceState,
    run_id: &str,
    events: &[serde_json::Value],
    final_content: &str,
    locale: Locale,
) {
    chat.start_assistant_message(run_id);
    for ev in events {
        // Replay always reconstructs the conversation being viewed — treat
        // it as foreground so live-follow behaves the same as it did live.
        apply_trace_event(chat, workspace, run_id, ev, locale);
    }
    chat.complete_run(run_id);
    // A persisted trace that ends mid-tool (run killed / process died between
    // `ToolCallStarted` and `ToolCallCompleted`) would otherwise replay a row
    // that pulses `running` forever in a conversation that ended days ago.
    chat.settle_orphan_tools(run_id);
    // Same authoritative promotion as the live `run_complete` path: overwrite
    // the trailing bubble with the history-authoritative answer and flag it
    // `is_final` so a turn that ended with text + a tool call still renders the
    // answer as a bubble, not a trapped step.
    chat.finalize_answer(run_id, final_content);
}

fn append_reasoning(chat: ChatState, summary: &str) {
    if summary.is_empty() {
        return;
    }

    chat.reasoning_text.update(|text: &mut String| {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(summary);
    });
}

/// Project core-authoritative context occupancy onto the composer gauge.
/// Reads `context_tokens` / `context_window` / `total_tokens` — the same field
/// names on both feeds: the terminal `run_complete` summary and the mid-run
/// `context_gauge` events emitted once per LLM call (so the gauge tracks a
/// long run live, including the drop right after a mid-run compaction).
/// Pure rendering (R4): core computes both sides; the panel only displays
/// them. No-op unless both occupancy and window are present, so legacy
/// payloads and runs with no LLM call leave the gauge hidden.
fn apply_context_gauge(chat: ChatState, summary: &serde_json::Value) {
    let used = summary
        .get("context_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let window = summary
        .get("context_window")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let total = summary
        .get("total_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if used > 0 && window > 0 {
        chat.context_usage.set(Some(ContextUsage {
            used_tokens: used,
            window_tokens: window,
            total_tokens: total,
            is_estimate: false,
        }));
    }
}

/// Project the run's cost + token split from the `run_complete` summary onto
/// the assistant bubble's meta line. Core prices the run (`estimated_cost_usd`
/// / `cost_status`) and splits the tokens (`token_breakdown`); the panel only
/// renders (R4). No-op when the summary carries neither a price nor a token
/// total — a cost-less run must show nothing, not "$0.00".
fn apply_run_cost(chat: ChatState, run_id: &str, summary: &serde_json::Value) {
    let usd = summary
        .get("estimated_cost_usd")
        .and_then(serde_json::Value::as_f64);
    let status = summary
        .get("cost_status")
        .and_then(|v| v.as_str())
        .map(String::from);
    let total_tokens = summary
        .get("total_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let breakdown = summary.get("token_breakdown");
    let field = |k: &str| {
        breakdown
            .and_then(|b| b.get(k))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    if usd.is_none() && total_tokens == 0 {
        return;
    }
    chat.set_run_cost(
        run_id,
        RunCost {
            usd,
            status,
            total_tokens,
            input_tokens: field("input"),
            output_tokens: field("output"),
            // Same object, same wire shape — these two were simply not read,
            // which left the panel unable to say whether a session was
            // re-billing its history at `cache_creation`.
            cache_read_tokens: field("cache_read"),
            cache_creation_tokens: field("cache_creation"),
        },
    );
}

/// Read `run_complete`'s authoritative `summary.plan`. `None` for a core that
/// predates the field or a run that never touched the scratchpad — both mean
/// "leave the live projection alone". Pure, so the wire-shape contract is
/// host-testable.
fn parse_summary_plan(summary: &serde_json::Value) -> Option<super::plan::PlanView> {
    serde_json::from_value(summary.get("plan")?.clone()).ok()
}

/// Project `run_complete`'s authoritative `summary.tool_summaries[]` into the
/// panel's settlement shape. Pure so the wire-shape contract is host-testable.
///
/// Entries without a `tool_id` are skipped: a settlement addresses a row by id,
/// and an id-less one could only ever create an unaddressable orphan.
fn parse_tool_settlements(summary: &serde_json::Value) -> Vec<ToolSettlement> {
    let Some(items) = summary
        .get("tool_summaries")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let tool_id = item.get("tool_id").and_then(|v| v.as_str())?;
            if tool_id.is_empty() {
                return None;
            }
            Some(ToolSettlement {
                tool_id: tool_id.to_string(),
                tool_name: item
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                duration_ms: item
                    .get("duration_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                success: item
                    .get("success")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
            })
        })
        .collect()
}

/// Back-fill tool *result* payloads for the run's failures from
/// `summary.errors[]`, so a failed call whose `tool_call_completed` mirror was
/// dropped still renders its error text in the card body / detail pane instead
/// of an empty "…".
///
/// Written in the same `{"Error":{"error":…}}` envelope the live path records,
/// so `tool_card::error_message` picks it up with no new branch. Never
/// overwrites a payload the live stream already captured — that one is richer.
fn backfill_tool_errors(workspace: WorkspaceState, run_id: &str, summary: &serde_json::Value) {
    let Some(items) = summary.get("errors").and_then(serde_json::Value::as_array) else {
        return;
    };
    for item in items {
        let (Some(tool_id), Some(error)) = (
            item.get("tool_id").and_then(|v| v.as_str()),
            item.get("error").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        if tool_id.is_empty() {
            continue;
        }
        let already_captured = workspace
            .get_tool_payload(run_id, tool_id)
            .is_some_and(|p| p.result.is_some());
        if already_captured {
            continue;
        }
        workspace.record_tool_result(
            run_id,
            tool_id,
            serde_json::json!({ "Error": { "error": error } }),
        );
    }
}

/// Adopt a queued run into `chat` only when nothing else is in flight there.
/// Extracted (like `resolve_target` below) so the guard is unit-testable
/// without a Leptos event dependency.
///
/// A `RunQueued` frame arrives BECAUSE the session is busy, so the run it
/// names is very often someone else's — a teammate, a second tab, a channel,
/// cron. Calling `start_assistant_message` unconditionally here (a run-START)
/// would sink the live run's plan capsule, add an empty bubble under it, and
/// re-point Stop at a run the user never sent. No bubble is needed anyway:
/// the queued indicator renders off `phase` alone (`messages.rs`'s queued
/// `<Show>` is a sibling of the Thinking one, not a bubble).
///
/// Why this is right in each case:
/// - **Own send:** the composer's `sessions.bind_run` never calls
///   `start_assistant_message`, so `active_run_id` is still `None` when this
///   frame lands → adopts → renders.
/// - **Foreign run while one is live:** `active_run_id` is `Some(other)` →
///   no adopt → `mark_queued` no-ops (it is already scoped to
///   `active_run_id`) → nothing is clobbered.
/// - **Idle conversation:** adopts. Coherent for a shared session — the
///   front of the lane is that conversation's next turn.
/// - It also removes the phantom-run risk from a steered send, since a steer
///   only happens while a run is live, which is exactly the no-adopt case.
fn apply_run_queued(chat: ChatState, run_id: &str, ahead: u16) {
    if chat.active_run_id.get_untracked().is_none() {
        chat.active_run_id.set(Some(run_id.to_string()));
    }
    chat.mark_queued(run_id, ahead);
}

/// Resolve which conversation's `ChatState` one run event should land on, and
/// maintain running/route bookkeeping. Returns the target `ChatState` plus
/// whether the resolved conversation is the active (foreground) one. That flag
/// is what gates any side effect on the single global `WorkspaceState`, so a
/// background conversation can never hijack the pane the user is looking at.
/// `None` = drop. Extracted so it's unit-testable without a Leptos event
/// dependency.
fn resolve_target(
    sessions: &SessionMap,
    singleton: ChatState,
    event_type: &str,
    run_id: &str,
    session_key: Option<&str>,
) -> Option<(ChatState, bool)> {
    let conv = match event_type {
        // New run, resolved in three steps — and the ORDER is the whole point:
        //
        // 1. The send path binds `run_id` → the conversation active at *send*
        //    time (authoritative when the user switches tabs before the run is
        //    accepted, I1).
        // 2. Otherwise this is a run THIS client did not start — a second
        //    Panel tab, another member of a project room, the CLI/TUI, a
        //    channel, cron, a resumed run. Route it by the identity the frame
        //    itself carries. `RunAccepted` has always carried `session_key`
        //    and `SessionMap::conv_for_session_key` has always existed; they
        //    were simply never joined here.
        // 3. Last resort, the foreground conversation — but only when it
        //    cannot be shown to belong to a DIFFERENT session. A conversation
        //    with no key yet (opened but never sent in) still qualifies, which
        //    is what keeps a legacy core (whose frames carry no `session_key`)
        //    working exactly as before.
        //
        //    Note what step 3 does NOT do: there is no arm here for a surface
        //    that registers no conversation at all. It needs an `active_conv()`
        //    like every other step, so such a surface resolves `None` for
        //    EVERY frame and receives no live turn whatsoever — no assistant
        //    bubble, no tool rows, no final answer, nothing logged. This
        //    comment used to claim the opposite, and the phone was exactly that
        //    surface for as long as it existed; the claim was the only thing a
        //    grep for the defect found. Registration is now the surface's job
        //    (`SessionMap::ensure_active` / `adopt_session`), pinned by
        //    `a_surface_that_registers_no_conversation_receives_no_frame` below
        //    and by `PhoneChat`'s own `the_phone_chat_router_registers_a_conversation`.

        //
        // Step 3 used to be unconditional, and that was the "two terminals
        // stepping on each other" defect: a foreign run's whole turn —
        // reasoning, tool rows, final answer — rendered into whatever
        // conversation the viewer happened to be reading, `bind_run` then
        // pinned every later frame of that run to it, and the `run_accepted`
        // arm below overwrote that tab's `session_key`, so the user's *next
        // message* went to somebody else's session. Reachable from a second
        // Panel tab, another member of a project room, the CLI, any channel,
        // and every cron tick.
        //
        // A run whose session is open in no tab now resolves to `None` and is
        // dropped. Nothing is lost by that: the sidebar dot still lights for
        // it (`stream.running_set_changed` is keyed by session and never
        // consulted this route), and the transcript arrives when the user
        // opens that session — where `hydrate_and_follow` binds the run if it
        // is still going, and the terminal `run.session_updated` re-hydrates
        // if it is not.
        // `run_queued` joins this arm because it is now a run's FIRST frame:
        // it can arrive before this client has any route for the run, and it
        // carries the same `session_key`. Every LATER frame still routes by
        // `route_lookup` alone — only the first one has nothing to look up.
        "run_accepted" | "run_queued" => sessions
            .route_lookup(run_id)
            .or_else(|| session_key.and_then(|sk| sessions.conv_for_session_key(sk)))
            .or_else(|| {
                let conv = sessions.active_conv()?;
                let open = sessions.meta(conv).and_then(|m| m.session_key);
                // Refuse ONLY what can be positively proved to belong
                // elsewhere: both keys known and different. Everything else —
                // a conversation with no key yet (a new chat before its first
                // send response), a frame with no key at all (a core predating
                // the field) — leaves the foreground as the only answer
                // available, which is what step 3 has always been for.
                match (open.as_deref(), session_key) {
                    (Some(open), Some(incoming)) if open != incoming => None,
                    _ => Some(conv),
                }
            }),
        // `reasoning` without a run_id: route to the active conversation too;
        // with a run_id it must follow that run's owning conversation like
        // every other event, so it doesn't bleed into whichever conversation
        // happens to be in the foreground.
        "reasoning" if run_id.is_empty() => sessions.active_conv(),
        _ => sessions.route_lookup(run_id),
    }?;
    // Only bind here when the send path hasn't already (route absent) — binding
    // twice would double-count `running` and leave a phantom dot.
    if matches!(event_type, "run_accepted" | "run_queued")
        && sessions.route_lookup(run_id).is_none()
    {
        sessions.bind_run(run_id, conv, session_key);
    }
    let target = sessions.chat_for(conv, singleton);
    let is_foreground = sessions.active_conv() == Some(conv);
    if matches!(event_type, "run_complete" | "run_error") {
        sessions.settle_run(run_id);
    }
    target.map(|chat| (chat, is_foreground))
}

/// Subscribe to `run.*` events and dispatch to `ChatState`. Tool args/results
/// are mirrored into [`WorkspaceState::tool_payloads`] so the workspace
/// pane can render real invocation details without an extra round-trip.
/// Returns the subscription ID for cleanup.
#[must_use]
pub fn subscribe_run_events(
    dashboard: &DashboardState,
    sessions: SessionMap,
    singleton: ChatState,
    workspace: WorkspaceState,
    i18n: I18nCtx,
) -> usize {
    let trace_runs = Arc::new(Mutex::new(HashSet::<String>::new()));
    // Owned Copy captured into the 'static event closure — used to drive
    // voice-loop TTS playback when a registered run completes.
    let dash = *dashboard;
    // Captured HERE rather than read inside the closure: that closure is
    // 'static and runs outside any reactive owner, where `use_context` has
    // nothing to read from. `UserDirectoryState` is `Copy` and its signals are
    // process-wide, so this handle keeps seeing later writes — including the
    // `users.me` fetch `MessageList` kicks off on first render. `use_context`
    // (not `expect_context`) because a mount without the directory must still
    // stream normally; it simply never renders a peer echo.
    let user_dir = use_context::<UserDirectoryState>();
    dashboard.subscribe_events(move |event: GatewayEvent| {
        if !event.topic.starts_with("run.") {
            return;
        }

        let data = &event.data;
        let trace_runs = trace_runs.clone();

        // Extract event type: prefer data.type, fallback to topic suffix (e.g. "run.run_accepted" -> "run_accepted")
        let event_type = data
            .get("type")
            .and_then(|t| t.as_str())
            .or_else(|| event.topic.strip_prefix("run."))
            .unwrap_or("");
        let run_id = data.get("run_id").and_then(|r| r.as_str()).unwrap_or("");
        let trace_enabled = !run_id.is_empty()
            && trace_runs
                .lock()
                .map(|runs| runs.contains(run_id))
                .unwrap_or(false);

        // `clarification_ended`: the terminal twin of `ask_user`, and the only
        // signal that a question is over (answered, superseded, expired, run
        // cancelled). Dropping the entry removes the card and releases the
        // composer's Enter hijack — in every window, including the ones that
        // never saw the answer. Keyed by session and carries no run_id, so it
        // must be handled before the run_id guard below.
        if event_type == "clarification_ended" {
            if let Some(session_key) = data.get("session_key").and_then(|s| s.as_str()) {
                dash.pending_clarifications
                    .update(|list| list.retain(|p| p.session_key != session_key));
            }
            return;
        }

        // `session_user_message`: another human's message became a transcript
        // row. Like `clarification_ended` above it is keyed by session and
        // carries no run_id — necessarily so: the run it belongs to is somebody
        // else's, which is the entire reason this client cannot already see the
        // message. Hence handled before the run_id guard.
        //
        // This closes the gap where a room peer watched an answer stream in
        // with no question above it: `RunAccepted` starts the assistant bubble
        // for a foreign run, but the user row behind it only arrived when the
        // turn ended and `run.session_updated` re-hydrated (the sidebar's
        // re-hydrate is suppressed while the session is running).
        if event_type == "session_user_message" {
            let Some(session_key) = data.get("session_key").and_then(|s| s.as_str()) else {
                return;
            };
            let author = data
                .get("author_user_id")
                .and_then(|a| a.as_str())
                .unwrap_or_default();
            let me = user_dir.and_then(|d| d.my_user_id.get_untracked());
            if !peer_message_is_renderable(author, me.as_deref()) {
                return;
            }
            // Routed by the identity the frame carries, and dropped when no
            // conversation holds that session — the same rule `resolve_target`
            // applies to a foreign run, and for the same reason: the transcript
            // arrives whole when the viewer opens the session.
            let Some(chat) = sessions
                .conv_for_session_key(session_key)
                .and_then(|conv| sessions.chat_for(conv, singleton))
            else {
                return;
            };
            chat.push_peer_user_message(
                data.get("seq")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                data.get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default(),
                author,
                // The same parser the history path feeds, so a live bubble and
                // its reloaded twin land on the same day separator.
                data.get("timestamp")
                    .and_then(|t| t.as_str())
                    .and_then(super::timeline::parse_wire_timestamp),
            );
            return;
        }

        // Guard: most events require a valid run_id to associate with a message
        if run_id.is_empty() && event_type != "reasoning" {
            return;
        }

        // `ask_user`: the agent is parked on a question. Handled before
        // `resolve_target` because the pending list is keyed by session, not by
        // conversation — the card must appear even for a run this client never
        // bound (a reconnect, another surface's run). The frame carries the
        // clarification key; the panel stores it and posts it straight back.
        if event_type == "ask_user" {
            let Some(session_key) = data.get("session_key").and_then(|s| s.as_str()) else {
                return;
            };
            let ask = PendingAskView {
                session_key: session_key.to_string(),
                question: data
                    .get("question")
                    .and_then(|q| q.as_str())
                    .unwrap_or_default()
                    .to_string(),
                options: data
                    .get("options")
                    .and_then(serde_json::Value::as_array)
                    .map(|opts| {
                        opts.iter()
                            .filter_map(|o| o.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                // Structured view. Absent on a core that predates it — the
                // card then renders from `question`/`options` above, exactly
                // as a plain-text channel does.
                questions: data
                    .get("questions")
                    .and_then(serde_json::Value::as_array)
                    .map(|qs| qs.iter().filter_map(parse_ask_question).collect())
                    .unwrap_or_default(),
                answered: data
                    .get("answered")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(0),
            };
            // One question per session, core-side (a second `ask_user`
            // supersedes the first) — mirror that here instead of stacking.
            dash.pending_clarifications.update(|list| {
                list.retain(|p| p.session_key != ask.session_key);
                list.push(ask);
            });
            return;
        }

        // Resolve the target conversation's ChatState (active = singleton
        // projection / background = live[conv]). `resolve_target` also reports
        // whether that conversation is the foreground one; the workspace pane
        // has no per-conversation surface to protect right now, so nothing
        // consumes it — any future write to the single global pane must.
        let session_key = data.get("session_key").and_then(|s| s.as_str());
        let Some((chat, _is_foreground)) =
            resolve_target(&sessions, singleton, event_type, run_id, session_key)
        else {
            return;
        };

        match event_type {
            "run_queued" => {
                // Backfill the key exactly like `run_accepted` does, for the
                // same reason: a brand-new conversation learns its
                // server-assigned key from whichever of the two arrives first,
                // and for a queued run that is this one.
                if let Some(sk) = data.get("session_key").and_then(|s| s.as_str()) {
                    if chat.session_key.get_untracked().is_none() {
                        chat.session_key.set(Some(sk.to_string()));
                    }
                }
                let ahead = data
                    .get("ahead")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                apply_run_queued(chat, run_id, u16::try_from(ahead).unwrap_or(u16::MAX));
            }
            "run_accepted" => {
                // BACKFILL, not assignment. A brand-new conversation learns
                // its server-assigned key here (the send path routed the run
                // before any key existed). But once a conversation has a key,
                // this frame can never rename it: `resolve_target` now only
                // hands us a conversation that either has no key yet or whose
                // key IS this frame's, so a write over a different value could
                // only be a routing bug re-entering through the back door —
                // and its symptom (the user's next message silently addressed
                // to another session) is the worst one in this file.
                if let Some(sk) = data.get("session_key").and_then(|s| s.as_str()) {
                    if chat.session_key.get_untracked().is_none() {
                        chat.session_key.set(Some(sk.to_string()));
                    }
                }
                chat.start_assistant_message(run_id);
                // Admission is the edge that ends the wait, and it cannot ride
                // on `start_assistant_message`: that early-returns once the
                // run's bubble exists, and the queued frame has already
                // created it. Without this the phase reads "queued" until the
                // first `turn_started` or token — the whole of model latency.
                chat.mark_admitted(run_id);
            }
            "reasoning" => {
                if let Some(content) = data.get("content").and_then(|c| c.as_str()) {
                    chat.reasoning_text
                        .update(|t: &mut String| t.push_str(content));
                }
            }
            "agent_trace" => {
                if let Ok(mut runs) = trace_runs.lock() {
                    runs.insert(run_id.to_string());
                }
                let Some(trace_event) = data.get("event") else {
                    return;
                };
                // Resolve the live locale per event (untracked — this closure
                // is outside any reactive scope), so a mid-session language
                // switch is honoured by subsequent narration.
                apply_trace_event(
                    chat,
                    workspace,
                    run_id,
                    trace_event,
                    i18n.get_locale_untracked(),
                );
            }
            "tool_start" => {
                if trace_enabled {
                    return;
                }
                let name = data
                    .get("tool_name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("tool");
                let tool_id = data.get("tool_id").and_then(|t| t.as_str()).unwrap_or("");
                chat.update_tool(run_id, tool_id, name, "running", None);
            }
            "tool_end" => {
                if trace_enabled {
                    return;
                }
                let tool_id = data.get("tool_id").and_then(|t| t.as_str()).unwrap_or("");
                let status = data
                    .get("result")
                    .and_then(|r| r.get("success"))
                    .and_then(serde_json::Value::as_bool)
                    .map(|ok| if ok { "completed" } else { "failed" })
                    .unwrap_or("completed");
                let duration = data.get("duration_ms").and_then(serde_json::Value::as_u64);
                chat.update_tool(run_id, tool_id, "", status, duration);
            }
            "response_chunk" => {
                // Live typewriter preview. When trace is active, authoritative
                // per-step text arrives via `agent_trace.text_emitted` (both
                // output modes) and overwrites this preview, so the `is_final`
                // chunk — in instant mode the whole-run buffered dump — is
                // dropped to avoid duplicating already-set step text. For
                // non-trace runs no text_emitted arrives, so the `is_final`
                // chunk is the only text source and must be kept.
                let is_final = data
                    .get("is_final")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if is_final && trace_enabled {
                    return;
                }
                let chunk_text = data
                    .get("delta")
                    .or_else(|| data.get("content"))
                    .and_then(|c| c.as_str());
                if let Some(text) = chunk_text {
                    chat.append_chunk(run_id, text);
                }
            }
            "model_resolved" => {
                let model = data
                    .get("model_info")
                    .and_then(|m| serde_json::from_value::<ModelInfo>(m.clone()).ok());
                if let Some(info) = model {
                    chat.set_model_info(run_id, info);
                }
            }
            "run_retrying" => {
                // Provider chain failed transiently; surface the retry under
                // the thinking indicator instead of leaving minutes of
                // silence. Cleared on the next chunk / run settle.
                let provider = data
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let attempt = data
                    .get("attempt")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as u32;
                let max_attempts = data
                    .get("max_attempts")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as u32;
                chat.set_provider_retry(ProviderRetryNotice {
                    provider,
                    attempt,
                    max_attempts,
                });
            }
            "context_gauge" => {
                // Mid-run occupancy update, once per LLM call. Field names
                // match the run_complete summary, so the same projector
                // applies; the terminal summary still lands last and stays
                // authoritative.
                apply_context_gauge(chat, data);
            }
            "run_complete" => {
                // The run is over: its trace-mode membership is no longer
                // needed. Without this prune the set grew by one entry per
                // run for the lifetime of the view.
                if let Ok(mut runs) = trace_runs.lock() {
                    runs.remove(run_id);
                }
                chat.complete_run(run_id);
                // Promote the harness-authoritative final answer into the
                // trailing bubble so it renders as the conversational reply —
                // even when the terminating turn also issued a tool call (which
                // would otherwise keep `is_final_answer` false and trap the
                // answer in the step strip). Mirrors `replay_run`'s overwrite.
                if let Some(final_text) = data
                    .get("summary")
                    .and_then(|s| s.get("final_response"))
                    .and_then(|v| v.as_str())
                {
                    chat.finalize_answer(run_id, final_text);
                }
                // Context gauge: core now ships the authoritative current
                // occupancy (`context_tokens`) and the per-model window
                // (`context_window`); the panel is a pure renderer (R4). The
                // run-cumulative `total_tokens` rides along for the tooltip.
                if let Some(summary) = data.get("summary") {
                    apply_context_gauge(chat, summary);
                    // Cost + token split for the bubble's meta line, and the
                    // right pane's `RunMetaInspector` breakdown behind it.
                    apply_run_cost(chat, run_id, summary);
                    // Authoritative tool outcomes: repair whatever the lossy
                    // `agent_trace` mirror dropped (see `reconcile_tools`).
                    chat.reconcile_tools(run_id, &parse_tool_settlements(summary));
                    backfill_tool_errors(workspace, run_id, summary);
                    // Same contract for the Todo strip: `summary.plan` is the
                    // core-latched terminal execution list, so a dropped
                    // `complete_item` frame no longer strands it mid-plan.
                    chat.settle_plan(parse_summary_plan(summary).as_ref());
                } else {
                    // No summary at all (older core): still sink a finished
                    // plan so it does not stay mounted into the next turn.
                    chat.settle_plan(None);
                }
                // Anything the summary did not name (older core, a run with no
                // timeline) still has to leave `running` — the run is over.
                chat.settle_orphan_tools(run_id);
                // Voice loop: if the mic button registered this run, speak the
                // final reply via the core TTS path → endpoint playback.
                if chat.take_speak_run(run_id) {
                    let text = chat.assistant_text_for_run(run_id);
                    if !text.trim().is_empty() {
                        super::voice_playback::speak(&dash, &chat, text);
                    }
                }
            }
            "run_error" => {
                if let Ok(mut runs) = trace_runs.lock() {
                    runs.remove(run_id);
                }
                let error = data
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("Unknown error");
                // The server already classified this failure and named the
                // bucket. Dropping the code here is what made Stop, a rejected
                // queued message, and an expired key all render as UNKNOWN.
                let error_code = data.get("error_code").and_then(|c| c.as_str());
                chat.fail_run(run_id, error, error_code);
                // `RunError` carries no summary, so there is nothing to
                // reconcile against — but the run is over, so any row still
                // `running` must stop pulsing (and stop ticking).
                chat.settle_orphan_tools(run_id);
                // A plan the failed run happened to finish still sinks; an
                // unfinished one stays mounted, which is what the user needs
                // to see after an error.
                chat.settle_plan(None);
            }
            _ => {} // Ignore unknown event types
        }
    })
}

#[cfg(test)]
mod projection_tests {
    use super::*;
    use crate::state::layout::WorkspaceState;
    use crate::views::chat::state::{ChatPhase, ChatState};
    use leptos::prelude::Owner;
    use serde_json::json;

    /// The case the frame exists for: another member of the room typed, and
    /// this viewer is not them.
    #[test]
    fn a_room_peers_message_renders() {
        assert!(peer_message_is_renderable("u-alice", Some("u-bob")));
    }

    /// The sender's own echo, in every tab they have open. This is the whole
    /// duplicate-suppression story — note it does NOT consult the run id,
    /// because this frame can outrun the `chat.send` response that would
    /// register it (see the predicate's doc).
    #[test]
    fn my_own_message_never_renders() {
        assert!(!peer_message_is_renderable("u-alice", Some("u-alice")));
    }

    /// Either half unknown ⇒ the comparison is unanswerable, so decline. Costs
    /// nothing beyond the pre-existing behavior (the message still lands when
    /// the turn ends and `run.session_updated` re-hydrates), whereas guessing
    /// "render" duplicates the sender's own bubble with nothing to clean it up.
    #[test]
    fn an_unanswerable_comparison_declines() {
        // `users.me` still in flight, or a caller with no P1 identity at all.
        assert!(!peer_message_is_renderable("u-alice", None));
        assert!(!peer_message_is_renderable("u-alice", Some("")));
        // Server-side this cannot happen (the frame's author is not optional),
        // but a client must not treat an absent author as "somebody else".
        assert!(!peer_message_is_renderable("", Some("u-bob")));
    }

    #[test]
    fn replay_run_rebuilds_intermediates_then_final_answer() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();

        let events = vec![
            json!({ "kind": "turn_started", "iteration": 1 }),
            json!({ "kind": "text_emitted", "iteration": 1, "stream": "step", "text": "looking" }),
            json!({ "kind": "tool_call_started", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "search", "input": { "q": "x" } } }),
            json!({ "kind": "tool_call_completed", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "search", "duration_ms": 3 },
                    "result": { "ok": true } }),
            json!({ "kind": "turn_started", "iteration": 2 }),
            json!({ "kind": "text_emitted", "iteration": 2, "stream": "final", "text": "raw final" }),
        ];

        replay_run(
            chat,
            ws,
            "run-1",
            &events,
            "AUTHORITATIVE ANSWER",
            Locale::default(),
        );

        let msgs = chat.messages.get_untracked();
        assert!(
            msgs.iter()
                .any(|m| m.is_intermediate && m.id.starts_with("intermediate-run-1-")),
            "expected an intermediate step bubble"
        );
        let final_bubble = msgs
            .iter()
            .find(|m| m.id == "assistant-run-1")
            .expect("final answer bubble");
        assert!(!final_bubble.is_intermediate);
        assert!(!final_bubble.is_streaming);
        assert_eq!(final_bubble.content, "AUTHORITATIVE ANSWER");
        assert!(ws.get_tool_payload("run-1", "t1").is_some());
    }

    #[test]
    fn apply_trace_event_builds_steps_and_payloads() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();

        // Simulate run_accepted which creates the placeholder message bubble
        // that begin_step / set_step_text require to exist.
        chat.start_assistant_message("run-1");

        let events = vec![
            json!({ "kind": "turn_started", "iteration": 1 }),
            json!({ "kind": "tool_call_started", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "search", "input": { "q": "rust" } } }),
            json!({ "kind": "tool_call_completed", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "search", "duration_ms": 5 },
                    "result": { "ok": true } }),
            json!({ "kind": "turn_started", "iteration": 2 }),
            json!({ "kind": "text_emitted", "iteration": 2, "stream": "final", "text": "done" }),
        ];
        for ev in &events {
            apply_trace_event(chat, ws, "run-1", ev, Locale::default());
        }

        let payload = ws.get_tool_payload("run-1", "t1").expect("payload");
        assert!(payload.args.is_some());
        assert!(payload.result.is_some());

        let msgs = chat.messages.get_untracked();
        let tagged: Vec<usize> = msgs.iter().filter_map(|m| m.iteration).collect();
        assert_eq!(tagged, vec![1, 2]);
        assert!(msgs
            .iter()
            .any(|m| m.iteration == Some(2) && m.content.contains("done")));
    }

    #[test]
    fn scratchpad_string_encoded_output_projects_plan_to_panel() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("run-1");

        // Real gateway wire shape: the harness serializes the scratchpad tool's
        // structured output as a JSON-encoded STRING inside `Success.output`,
        // so the snapshot is nested one JSON level deeper than a plain object.
        // The Todo-panel projection must decode the string before reading it.
        let output_str = concat!(
            "{\"content\":\"Plan:...\",\"message\":\"Plan set with 3 items\",",
            "\"snapshot\":{\"complete\":false,\"objective\":null,\"items\":[",
            "{\"status\":\"pending\",\"text\":\"step one\"},",
            "{\"status\":\"in_progress\",\"text\":\"step two\"},",
            "{\"status\":\"completed\",\"text\":\"step three\"}]},\"success\":true}"
        );
        let ev = json!({
            "kind": "tool_call_completed", "iteration": 1,
            "call": { "tool_id": "s1", "tool_name": "scratchpad", "duration_ms": 13,
                      "input": { "action": "set_plan" } },
            "result": { "Success": { "output": output_str } }
        });
        apply_trace_event(chat, ws, "run-1", &ev, Locale::default());

        let plan = chat
            .plan
            .get_untracked()
            .expect("plan must be Some after a string-encoded snapshot");
        assert_eq!(plan.total(), 3);
        assert_eq!(plan.done_count(), 1);
        assert!(plan.has_content());
    }

    fn scratchpad_event(action: &str, items: &[(&str, &str)]) -> serde_json::Value {
        // Build the real wire shape: Success.output is a JSON-encoded STRING
        // whose `snapshot` carries the plan.
        let items_json: Vec<serde_json::Value> = items
            .iter()
            .map(|(status, text)| json!({ "status": status, "text": text }))
            .collect();
        let complete = !items.is_empty() && items.iter().all(|(s, _)| *s == "completed");
        let snapshot = json!({ "complete": complete, "objective": "Obj", "items": items_json });
        let output = serde_json::to_string(&json!({
            "success": true, "message": "ok", "snapshot": snapshot
        }))
        .unwrap();
        json!({
            "kind": "tool_call_completed", "iteration": 1,
            "call": { "tool_id": "s1", "tool_name": "scratchpad", "duration_ms": 1,
                      "input": { "action": action } },
            "result": { "Success": { "output": output } }
        })
    }

    fn archive_count(chat: &ChatState) -> usize {
        chat.messages
            .with(|m| m.iter().filter(|x| x.plan_archive.is_some()).count())
    }

    #[test]
    fn set_plan_supersede_archives_worked_prior() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("r1");
        // plan A, then start an item (activity), then a fresh set_plan B
        apply_trace_event(
            chat,
            ws,
            "r1",
            &scratchpad_event("set_plan", &[("in_progress", "a")]),
            Locale::default(),
        );
        apply_trace_event(
            chat,
            ws,
            "r1",
            &scratchpad_event("set_plan", &[("pending", "b")]),
            Locale::default(),
        );
        assert_eq!(archive_count(&chat), 1, "worked prior plan A sinks");
        let plan = chat.plan.get_untracked().expect("new plan B shown");
        assert_eq!(plan.items[0].text, "b");
    }

    #[test]
    fn set_plan_supersede_skips_pristine_prior() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("r1");
        // plan A pristine (all pending), immediately replaced → silent
        apply_trace_event(
            chat,
            ws,
            "r1",
            &scratchpad_event("set_plan", &[("pending", "a")]),
            Locale::default(),
        );
        apply_trace_event(
            chat,
            ws,
            "r1",
            &scratchpad_event("set_plan", &[("pending", "b")]),
            Locale::default(),
        );
        assert_eq!(
            archive_count(&chat),
            0,
            "pristine prior A is silently replaced"
        );
        assert_eq!(chat.plan.get_untracked().unwrap().items[0].text, "b");
    }

    #[test]
    fn clear_archives_completed_then_hides() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("r1");
        apply_trace_event(
            chat,
            ws,
            "r1",
            &scratchpad_event("set_plan", &[("completed", "a")]),
            Locale::default(),
        );
        apply_trace_event(
            chat,
            ws,
            "r1",
            &scratchpad_event("clear", &[]),
            Locale::default(),
        );
        assert_eq!(archive_count(&chat), 1, "completed plan sinks on clear");
        assert!(
            chat.plan.get_untracked().is_none(),
            "panel hidden after clear"
        );
    }

    #[test]
    fn start_item_update_does_not_archive() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("r1");
        apply_trace_event(
            chat,
            ws,
            "r1",
            &scratchpad_event("set_plan", &[("pending", "a"), ("pending", "b")]),
            Locale::default(),
        );
        // a same-plan update (start_item) must NOT sink a capsule
        apply_trace_event(
            chat,
            ws,
            "r1",
            &scratchpad_event("start_item", &[("in_progress", "a"), ("pending", "b")]),
            Locale::default(),
        );
        assert_eq!(
            archive_count(&chat),
            0,
            "in-place update is not a supersede"
        );
    }

    #[test]
    fn replay_reconstructs_same_archive_capsules() {
        let owner = Owner::new();
        owner.set();
        // Live path
        let live = ChatState::new();
        let ws1 = WorkspaceState::new();
        live.start_assistant_message("r1");
        apply_trace_event(
            live,
            ws1,
            "r1",
            &scratchpad_event("set_plan", &[("completed", "a")]),
            Locale::default(),
        );
        live.start_assistant_message("r2"); // next-turn sink of completed A
        let live_caps = live.messages.with(|m| {
            m.iter()
                .filter_map(|x| x.plan_archive.clone())
                .collect::<Vec<_>>()
        });

        // Replay path: same two runs reconstructed via replay_run
        let rep = ChatState::new();
        let ws2 = WorkspaceState::new();
        replay_run(
            rep,
            ws2,
            "r1",
            &[scratchpad_event("set_plan", &[("completed", "a")])],
            "done",
            Locale::default(),
        );
        replay_run(rep, ws2, "r2", &[], "next", Locale::default());
        let rep_caps = rep.messages.with(|m| {
            m.iter()
                .filter_map(|x| x.plan_archive.clone())
                .collect::<Vec<_>>()
        });

        assert_eq!(live_caps.len(), 1, "live sinks one capsule");
        assert_eq!(rep_caps.len(), 1, "replay reconstructs the same one");
        assert_eq!(
            live_caps, rep_caps,
            "live and replay capsules are identical"
        );
    }

    #[test]
    fn run_complete_projects_core_context_occupancy_to_gauge() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();

        // Core ships authoritative occupancy + per-model window on the summary;
        // the panel must project them straight onto the gauge.
        let summary = json!({
            "context_tokens": 42_000,
            "context_window": 200_000,
            "total_tokens": 55_000,
        });
        super::apply_context_gauge(chat, &summary);

        let usage = chat.context_usage.get_untracked().expect("gauge published");
        assert_eq!(usage.used_tokens, 42_000);
        assert_eq!(usage.window_tokens, 200_000);
        assert_eq!(usage.total_tokens, 55_000);
    }

    /// A surface that registers no conversation receives no frame at all.
    ///
    /// This is the mechanism, stated so it cannot be "fixed" by a silent
    /// fallback: with nothing in `SessionMap`, all three of `resolve_target`'s
    /// steps come up empty and the dispatcher returns before touching
    /// `ChatState`. No assistant bubble, no tool rows, no final answer, nothing
    /// logged — which is precisely what the phone did for as long as it
    /// existed, because `ChatSidebar` (the only thing in the crate that opened
    /// a conversation) is mounted behind `not_phone`.
    ///
    /// The answer is registration at the surface — `SessionMap::ensure_active`
    /// at mount, `adopt_session` when a session is picked — NOT a fourth step
    /// here. A fallback that invents a target is the "foreground hijack"
    /// defect this function was rewritten to remove: it would render a foreign
    /// run's whole turn into whatever the viewer happens to be reading and send
    /// their next message to somebody else's session.
    #[test]
    fn a_surface_that_registers_no_conversation_receives_no_frame() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();

        for (kind, run, key) in [
            ("run_accepted", "run-a", Some("sk-a")),
            ("response_chunk", "run-a", None),
            ("agent_trace", "run-a", None),
            ("run_complete", "run-a", None),
        ] {
            assert!(
                resolve_target(&sessions, singleton, kind, run, key).is_none(),
                "{kind} resolved a target with no conversation registered"
            );
        }

        // One `ensure_active` is the whole difference.
        sessions.ensure_active(singleton, "agent-a", || "New chat".into());
        assert!(
            resolve_target(&sessions, singleton, "run_accepted", "run-a", Some("sk-a")).is_some(),
            "a registered surface must be able to route its own turn"
        );
    }

    #[test]
    fn resolve_target_routes_background_run_to_registry() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();
        let a = sessions.open_conversation("agent-a", "A");
        let b = sessions.open_conversation("agent-b", "B");

        // A active, accept run-a; switch to B.
        sessions.activate(singleton, a);
        let t = resolve_target(&sessions, singleton, "run_accepted", "run-a", Some("sk-a"));
        assert_eq!(
            t.as_ref().map(|(c, _)| c.agent_id.get_untracked()),
            Some(singleton.agent_id.get_untracked())
        );
        assert_eq!(
            t.map(|(_, is_fg)| is_fg),
            Some(true),
            "A is foreground at send time"
        );
        sessions.activate(singleton, b);

        // Background chunk should route to A's live state, not the singleton (B).
        let (bg, is_fg) = resolve_target(&sessions, singleton, "response_chunk", "run-a", None)
            .expect("routed to background A");
        assert!(!is_fg, "A is backgrounded once B is active");
        let a_bg = sessions.chat_for(a, singleton).expect("A background");
        // Same background A instance (Copy makes signal identity comparison
        // awkward — compare append effects instead).
        bg.start_assistant_message("run-a");
        bg.append_chunk("run-a", "x");
        assert_eq!(a_bg.assistant_text_for_run("run-a"), "x");
        assert!(singleton.assistant_text_for_run("run-a").is_empty());

        // settle clears running.
        resolve_target(&sessions, singleton, "run_complete", "run-a", None);
        assert!(!sessions.is_running(a));
    }

    #[test]
    fn run_accepted_honors_send_time_binding_after_tab_switch() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();
        let a = sessions.open_conversation("agent-a", "A");
        let b = sessions.open_conversation("agent-b", "B");

        // User sends in A (send path binds the run to A), then immediately
        // switches to B before `run_accepted` arrives.
        sessions.activate(singleton, a);
        sessions.bind_run("run-a", a, Some("sk-a"));
        sessions.activate(singleton, b);

        // run_accepted must resolve to A (send-time truth), not B (foreground),
        // and must NOT double-count A's running refcount.
        let (t, is_fg) =
            resolve_target(&sessions, singleton, "run_accepted", "run-a", Some("sk-a"))
                .expect("resolved to A's background state");
        assert!(!is_fg, "A is not foreground — B is active");
        let a_bg = sessions.chat_for(a, singleton).expect("A background");
        t.start_assistant_message("run-a");
        t.append_chunk("run-a", "hi");
        assert_eq!(a_bg.assistant_text_for_run("run-a"), "hi");
        assert!(
            singleton.assistant_text_for_run("run-a").is_empty(),
            "B untouched"
        );

        // A single settle clears running → no phantom dot from a double bind.
        assert!(sessions.is_running(a));
        resolve_target(&sessions, singleton, "run_complete", "run-a", None);
        assert!(
            !sessions.is_running(a),
            "one settle clears it → bound exactly once"
        );
    }

    // ---- end-of-run tool reconciliation ---------------------------------
    //
    // The `agent_trace` mirror is best-effort by construction
    // (`AgentTraceEmitSink` = bounded mpsc + `try_send`, drops on overflow), so
    // these cover what the panel must repair from the authoritative
    // `run_complete` summary.

    fn status_of(chat: &ChatState, tool_id: &str) -> Option<(String, Option<u64>)> {
        chat.messages.with_untracked(|msgs| {
            msgs.iter()
                .flat_map(|m| m.tool_calls.iter())
                .find(|t| t.tool_id == tool_id)
                .map(|t| (t.status.clone(), t.duration_ms))
        })
    }

    fn tool_rows(chat: &ChatState) -> usize {
        chat.messages
            .with_untracked(|msgs| msgs.iter().map(|m| m.tool_calls.len()).sum())
    }

    #[test]
    fn parse_tool_settlements_reads_the_wire_shape_and_skips_idless() {
        let summary = json!({ "tool_summaries": [
            { "tool_id": "t1", "tool_name": "bash", "emoji": "❯", "duration_ms": 120, "success": true },
            { "tool_id": "t2", "tool_name": "web_search", "emoji": "🔍", "duration_ms": 7, "success": false },
            { "tool_id": "", "tool_name": "ghost", "duration_ms": 1, "success": true },
            { "tool_name": "no_id_at_all", "duration_ms": 1, "success": true },
        ]});
        let out = parse_tool_settlements(&summary);
        assert_eq!(out.len(), 2, "id-less entries are unaddressable → skipped");
        assert_eq!(out[0].tool_id, "t1");
        assert_eq!(out[0].duration_ms, 120);
        assert!(out[0].success);
        assert!(!out[1].success);
        // No summary / wrong shape degrades to empty rather than panicking.
        assert!(parse_tool_settlements(&json!({})).is_empty());
        assert!(parse_tool_settlements(&json!({ "tool_summaries": 3 })).is_empty());
    }

    /// The headline repair: the tool's `tool_call_completed` mirror frame was
    /// dropped, so the row is stuck `running` with a live elapsed timer — until
    /// `run_complete`'s authoritative summary settles it.
    #[test]
    fn run_complete_summary_settles_a_dropped_completion() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("r1");
        apply_trace_event(
            chat,
            ws,
            "r1",
            &json!({ "kind": "tool_call_started", "iteration": 1,
                     "call": { "tool_id": "t1", "tool_name": "bash", "input": { "cmd": "ls" } } }),
            Locale::default(),
        );
        assert_eq!(status_of(&chat, "t1").unwrap().0, "running");

        // ...`tool_call_completed` never arrives (dropped by the bounded mirror).
        let summary = json!({ "tool_summaries": [
            { "tool_id": "t1", "tool_name": "bash", "duration_ms": 4200, "success": true }
        ]});
        chat.reconcile_tools("r1", &parse_tool_settlements(&summary));
        chat.settle_orphan_tools("r1");

        assert_eq!(
            status_of(&chat, "t1"),
            Some(("completed".to_string(), Some(4200))),
            "authoritative summary settles the row and supplies its duration"
        );
        assert_eq!(tool_rows(&chat), 1, "repair must not duplicate the row");
    }

    /// The mirror can just as easily drop the *start* frame, in which case the
    /// call was never visible at all — the summary is the only evidence it ran.
    #[test]
    fn run_complete_summary_creates_a_row_whose_start_was_dropped() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");

        let summary = json!({ "tool_summaries": [
            { "tool_id": "ghost", "tool_name": "file_read", "duration_ms": 12, "success": false }
        ]});
        chat.reconcile_tools("r1", &parse_tool_settlements(&summary));

        assert_eq!(
            status_of(&chat, "ghost"),
            Some(("failed".to_string(), Some(12)))
        );
        assert_eq!(tool_rows(&chat), 1);
    }

    /// `run_error` carries no summary at all, so there is nothing to reconcile
    /// against — but the run is over, so the row must stop pulsing.
    #[test]
    fn a_run_that_errors_settles_survivors_to_unknown() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("r1");
        apply_trace_event(
            chat,
            ws,
            "r1",
            &json!({ "kind": "tool_call_started", "iteration": 1,
                     "call": { "tool_id": "t1", "tool_name": "bash", "input": {} } }),
            Locale::default(),
        );
        chat.settle_orphan_tools("r1");
        assert_eq!(
            status_of(&chat, "t1").unwrap().0,
            crate::views::chat::state::TOOL_STATUS_UNKNOWN,
            "unknown, not a fabricated success"
        );
    }

    #[test]
    fn error_backfill_never_overwrites_a_captured_result() {
        let owner = Owner::new();
        owner.set();
        let ws = WorkspaceState::new();
        // t1's real result already landed live; t2's did not.
        ws.record_tool_result("r1", "t1", json!({ "Success": { "output": "fine" } }));
        let summary = json!({ "errors": [
            { "tool_id": "t1", "tool_name": "bash", "error": "late and wrong" },
            { "tool_id": "t2", "tool_name": "file_read", "error": "no such file" },
        ]});
        backfill_tool_errors(ws, "r1", &summary);

        assert_eq!(
            ws.get_tool_payload("r1", "t1").and_then(|p| p.result),
            Some(json!({ "Success": { "output": "fine" } })),
            "the live-captured payload is richer and must win"
        );
        assert_eq!(
            ws.get_tool_payload("r1", "t2")
                .and_then(|p| p.result)
                .and_then(|r| crate::components::tool_card::error_message(&r)),
            Some("no such file".to_string()),
            "the missing one is back-filled in the envelope the card already reads"
        );
    }

    /// Replay reconstructs from the *persisted* trace, which is complete — but a
    /// run killed between `ToolCallStarted` and `ToolCallCompleted` persists a
    /// half-open call that would otherwise replay as forever-running.
    #[test]
    fn replay_settles_a_trace_that_ends_mid_tool() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        let events = vec![
            json!({ "kind": "turn_started", "iteration": 1 }),
            json!({ "kind": "tool_call_started", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "bash", "input": {} } }),
        ];
        replay_run(chat, ws, "r1", &events, "killed", Locale::default());
        assert_eq!(
            status_of(&chat, "t1").unwrap().0,
            crate::views::chat::state::TOOL_STATUS_UNKNOWN
        );
    }

    // ── Multi-client routing (two terminals on one thread) ──────────────

    /// The defect the `run_accepted` fallback used to cause.
    ///
    /// A run this client did not start — a second Panel tab, another member of
    /// a project room, the CLI/TUI, a channel, every cron tick — arrives with
    /// a `run_id` no local route knows. The old fallback handed it the
    /// **foreground** conversation, so somebody else's turn rendered into
    /// whatever the viewer happened to be reading, and one arm later renamed
    /// that tab's `session_key`, sending the user's *next message* to the
    /// foreign session.
    ///
    /// Asserted on the resolved conversation and on the resulting route — not
    /// on the predicate having been consulted.
    #[test]
    fn a_foreign_run_routes_by_its_own_session_key_not_by_what_you_are_reading() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();

        let reading = sessions.open_conversation("agent-a", "reading");
        let other = sessions.open_conversation("agent-b", "other");
        // Both addressable AND both activated at least once — exactly what
        // `ChatSidebar::on_select_session` does (`open_conversation` →
        // `activate` → `set_session_key`). The activation matters: it is what
        // materialises a conversation's background `ChatState`, and without it
        // `chat_for` has nothing to hand back.
        sessions.activate(singleton, other);
        sessions.set_session_key(other, "sk-other");
        sessions.activate(singleton, reading);
        sessions.set_session_key(reading, "sk-reading");

        let (target, is_fg) = resolve_target(
            &sessions,
            singleton,
            "run_accepted",
            "run-foreign",
            Some("sk-other"),
        )
        .expect("a run on an open session resolves to that session's tab");
        assert!(!is_fg, "the other conversation is not the foreground one");
        target.start_assistant_message("run-foreign");
        target.append_chunk("run-foreign", "not yours");
        assert!(
            singleton.assistant_text_for_run("run-foreign").is_empty(),
            "the conversation being read must be untouched by a foreign run"
        );
        assert_eq!(
            sessions.route_lookup("run-foreign"),
            Some(other),
            "and the run must be pinned to the session it belongs to"
        );

        // A run on a session no tab is showing is dropped outright.
        assert!(
            resolve_target(
                &sessions,
                singleton,
                "run_accepted",
                "run-elsewhere",
                Some("sk-not-open"),
            )
            .is_none(),
            "a run whose session is open nowhere has no conversation to render \
             into — dropping it is the only answer that corrupts none"
        );
        assert!(
            sessions.route_lookup("run-elsewhere").is_none(),
            "and a dropped run must not be bound to anything"
        );
    }

    /// Joining a live turn and the live `run_accepted` frame can open the same
    /// bubble, in an order nobody controls: the `chat.history` response and the
    /// `RunAccepted` event travel one socket but are written by different arms
    /// of the dispatch `select!`. Whichever lands second must be a no-op.
    #[test]
    fn opening_one_runs_bubble_twice_leaves_one_bubble() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();

        // Order A: the joiner binds first, then the live frame arrives.
        chat.start_assistant_message("run-x");
        chat.append_chunk("run-x", "partial");
        chat.start_assistant_message("run-x");

        let bubbles = chat
            .messages
            .with_untracked(|msgs| msgs.iter().filter(|m| m.id == "assistant-run-x").count());
        assert_eq!(bubbles, 1, "a run has exactly one assistant bubble");
        assert_eq!(
            chat.assistant_text_for_run("run-x"),
            "partial",
            "and the second open must not blank the text already streamed \
             into it"
        );
    }

    /// The narrowed fallback still admits the two shapes that legitimately
    /// need it, so a brand-new conversation and a core predating `session_key`
    /// on this frame behave exactly as before.
    #[test]
    fn the_foreground_fallback_survives_for_an_unclaimed_conversation() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();
        let fresh = sessions.open_conversation("agent-a", "fresh");
        sessions.activate(singleton, fresh);

        // No key yet ⇒ nothing can prove the frame belongs elsewhere. This is
        // the first turn of a new chat, before any send response landed.
        assert!(
            resolve_target(
                &sessions,
                singleton,
                "run_accepted",
                "run-first",
                Some("sk-brand-new"),
            )
            .is_some(),
            "an unclaimed foreground conversation still accepts the frame"
        );
        assert_eq!(sessions.route_lookup("run-first"), Some(fresh));

        // A frame carrying no session key at all names no other session, so
        // the foreground remains the only available answer.
        let sessions2 = crate::state::sessions::SessionMap::new();
        let singleton2 = ChatState::new();
        let conv = sessions2.open_conversation("agent-a", "legacy");
        sessions2.set_session_key(conv, "sk-legacy");
        sessions2.activate(singleton2, conv);
        assert!(
            resolve_target(&sessions2, singleton2, "run_accepted", "run-legacy", None).is_some(),
        );
    }

    /// `run_queued` is now a run's FIRST frame, so it needs the same
    /// three-step resolution `run_accepted` has: route, then the session key
    /// the frame carries, then the foreground only when nothing proves it
    /// belongs elsewhere. Without this it falls through to `route_lookup`,
    /// finds nothing, and a queued run is invisible even in the tab that
    /// started it.
    #[test]
    fn run_queued_resolves_like_run_accepted() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();
        let a = sessions.open_conversation("agent-a", "A");
        sessions.activate(singleton, a);

        assert!(
            resolve_target(&sessions, singleton, "run_queued", "run-a", Some("sk-a")).is_some(),
            "a queued run must reach the conversation that started it"
        );
    }

    /// A queued frame for a session this client can prove belongs elsewhere is
    /// dropped, not painted into whatever the viewer happens to be reading —
    /// the same defect the unconditional `run_accepted` fallback caused.
    #[test]
    fn run_queued_for_a_foreign_session_is_dropped() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();

        let reading = sessions.open_conversation("agent-a", "reading");
        let other = sessions.open_conversation("agent-b", "other");
        // Both addressable AND both activated at least once — activation is
        // what materialises a conversation's background `ChatState`, and
        // without it `chat_for` has nothing to hand back.
        sessions.activate(singleton, other);
        sessions.set_session_key(other, "sk-other");
        sessions.activate(singleton, reading);
        sessions.set_session_key(reading, "sk-reading");

        assert!(
            resolve_target(
                &sessions,
                singleton,
                "run_queued",
                "run-x",
                Some("sk-somebody-else"),
            )
            .is_none(),
            "a queued run whose session is open in no tab must be dropped"
        );
    }

    /// The server already classified this failure and named the bucket.
    /// Passing only the prose is what left Stop rendering as an UNKNOWN error
    /// banner.
    #[test]
    fn run_error_forwards_the_servers_error_code() {
        use crate::views::chat::state::ChatSendErrorCode;

        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("run-a");
        chat.fail_run("run-a", "task cancelled", Some("CANCELLED"));

        assert_eq!(
            chat.send_error.get_untracked().map(|e| e.code),
            Some(ChatSendErrorCode::Cancelled)
        );
    }

    /// The defect this guards: a `RunQueued` frame arrives BECAUSE the
    /// session is busy, so it usually names someone else's run. Adopting it
    /// unconditionally used to call `start_assistant_message` — a run-START —
    /// which sinks the live run's plan capsule, adds an empty bubble under
    /// it, and re-points Stop at a run the user never sent.
    ///
    /// Falsified: restoring the unconditional `chat.start_assistant_message`
    /// call inside `apply_run_queued` turns this RED — `active_run_id`
    /// flips to the foreign run and the message count grows by one — and the
    /// failing assertions name themselves.
    #[test]
    fn run_queued_for_a_foreign_run_leaves_the_live_run_untouched() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();
        let conv = sessions.open_conversation("agent-a", "A");
        sessions.activate(singleton, conv);

        // A run is already live in this conversation.
        singleton.start_assistant_message("run-1");
        let msgs_before = singleton.messages.get_untracked().len();

        let (chat, _is_fg) =
            resolve_target(&sessions, singleton, "run_queued", "run-2", Some("sk-a"))
                .expect("resolves to the same conversation — same session");
        apply_run_queued(chat, "run-2", 3);

        assert_eq!(
            chat.active_run_id.get_untracked(),
            Some("run-1".to_string()),
            "the foreign queued run must not steal active_run_id from the live one"
        );
        assert_eq!(
            chat.messages.get_untracked().len(),
            msgs_before,
            "no bubble may be added for a run this conversation did not adopt"
        );
        assert_eq!(
            chat.phase.get_untracked(),
            ChatPhase::Thinking,
            "the live run's phase must not be repainted as queued"
        );
    }

    /// The complementary case: nothing else is in flight, so the queued run
    /// IS this conversation's next turn and must be adopted and rendered.
    #[test]
    fn run_queued_on_an_idle_conversation_adopts_and_renders() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();
        let conv = sessions.open_conversation("agent-a", "A");
        sessions.activate(singleton, conv);

        let (chat, _is_fg) =
            resolve_target(&sessions, singleton, "run_queued", "run-1", Some("sk-a"))
                .expect("a new conversation with no key yet resolves to the foreground");
        apply_run_queued(chat, "run-1", 0);

        assert_eq!(
            chat.active_run_id.get_untracked(),
            Some("run-1".to_string())
        );
        assert_eq!(chat.phase.get_untracked(), ChatPhase::Queued { ahead: 0 });
    }
}

#[cfg(test)]
mod tests {
    use super::provider_usage_pct;
    use serde_json::json;

    #[test]
    fn provider_usage_pct_uses_canonical_formula() {
        // 870 / (100 + 870) ≈ 89.7% → 90 — the same number the TUI status
        // bar and the core rollup show for this call.
        let ev = json!({
            "kind": "provider_usage",
            "agent_id": "root",
            "input_tokens": 100,
            "output_tokens": 10,
            "cache_read_tokens": 870,
            "cache_creation_tokens": 30,
            "thinking_tokens": null
        });
        assert_eq!(provider_usage_pct(&ev), Some(90));
    }

    #[test]
    fn provider_usage_pct_cold_write_is_zero_not_unknown() {
        let ev = json!({
            "kind": "provider_usage",
            "input_tokens": 500,
            "cache_read_tokens": 0,
            "cache_creation_tokens": 500
        });
        assert_eq!(provider_usage_pct(&ev), Some(0));
    }

    #[test]
    fn provider_usage_pct_none_without_cache_activity() {
        // Cache-less providers must not surface a misleading 0%.
        let ev = json!({
            "kind": "provider_usage",
            "input_tokens": 500,
            "cache_read_tokens": null,
            "cache_creation_tokens": null
        });
        assert_eq!(provider_usage_pct(&ev), None);
    }
}
