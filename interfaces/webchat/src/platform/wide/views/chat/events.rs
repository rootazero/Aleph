//! Maps Gateway streaming events (run.*) to `ChatState` mutations.

use super::state::{
    ChatState, ContextUsage, ModelInfo, ProviderRetryNotice, RunCost, ToolSettlement,
};
use crate::context::{DashboardState, GatewayEvent};
use crate::i18n::{td_string, I18nCtx, Locale};
use crate::state::layout::WorkspaceState;
use crate::state::notifications::PendingAskView;
use crate::state::sessions::SessionMap;
use leptos::prelude::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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
        _ => {}
    }
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
        // New run: the send path binds run_id → the conversation active at
        // *send* time (authoritative when the user switches tabs before the
        // run is accepted, I1), so prefer that route. Fall back to the active
        // conversation only for runs with no client send (e.g. daemon-initiated).
        "run_accepted" => sessions
            .route_lookup(run_id)
            .or_else(|| sessions.active_conv()),
        // `reasoning` without a run_id: route to the active conversation too;
        // with a run_id it must follow that run's owning conversation like
        // every other event, so it doesn't bleed into whichever conversation
        // happens to be in the foreground.
        "reasoning" if run_id.is_empty() => sessions.active_conv(),
        _ => sessions.route_lookup(run_id),
    }?;
    // Only bind here when the send path hasn't already (route absent) — binding
    // twice would double-count `running` and leave a phantom dot.
    if event_type == "run_accepted" && sessions.route_lookup(run_id).is_none() {
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
            "run_accepted" => {
                if let Some(sk) = data.get("session_key").and_then(|s| s.as_str()) {
                    chat.session_key.set(Some(sk.to_string()));
                }
                chat.start_assistant_message(run_id);
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
                        super::voice_playback::speak(&dash, text);
                    }
                }
            }
            "run_error" => {
                let error = data
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("Unknown error");
                chat.fail_run(run_id, error);
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
    use crate::views::chat::state::ChatState;
    use leptos::prelude::Owner;
    use serde_json::json;

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
}
