//! `StreamEvent` → `AppState` mutations + quit signal.
//!
//! Pulled out of [`mod`] to keep the orchestrator file under the 1 kLOC
//! soft cap. Lives in a second `impl AppState { … }` block.

use std::time::{Duration, Instant};

use aleph_protocol::{
    summarize_tool_input, AgentTracePresentationPreset, AgentTraceToolResult, AskUserQuestion,
    StreamEvent,
};

use super::super::slash::ToolProgressMode;
use super::{Action, AppState, AskDialogView, ChatMessage};

/// Bound on how many `(run_id, session_key)` pairs [`AppState`] remembers,
/// learned one per `RunAccepted`. A background/cron/subagent-heavy install
/// can mint many runs while one TUI screen stays open for hours; this keeps
/// that memory bounded. Eviction is FIFO and fails open: the oldest pair
/// falls out and that run_id reverts to "unknown" — which
/// `AppState::frame_belongs_here` already keeps rather than drops. That is
/// the safe direction: capping this can let a few stragglers back in, it can
/// never cause a frame that is actually ours to be dropped.
const RUN_SESSION_CAP: usize = 256;

/// The run id `handle_gateway_event`'s cross-session guard should check for
/// `event`, or `None` for a frame the guard does not apply to.
///
/// Default is GUARDED: `StreamEvent::run_id()` is an exhaustive match in
/// shared/protocol (no wildcard arm), so a tenth run-scoped variant added
/// there is a compile error until someone decides what it returns, and
/// inherits this guard the moment they do — nothing here has to be told
/// about it by name. Only one variant opts out below, and it says why.
/// `ClarificationEnded` already self-exempts: `run_id()` returns `""` for it
/// (session-keyed, not run-keyed, by the protocol's own design), which the
/// emptiness check turns into `None` without a named case here.
fn run_scoped_id(event: &StreamEvent) -> Option<&str> {
    match event {
        // Keyed by `session_key` — the clarification registry's actual
        // routing key — not by which run started it. A parked question
        // arguably belongs on every open screen until it is answered (R5:
        // AI comes to you), which is a different judgement call from "this
        // run's assistant text leaked into a transcript that isn't its
        // session's." Scoping AskUser by run/session is left to its own
        // task rather than folded into this guard by accident.
        StreamEvent::AskUser { .. } => None,
        other => {
            let id = other.run_id();
            (!id.is_empty()).then_some(id)
        }
    }
}

/// Flatten an `AskUser` frame into the view the dialog overlay renders.
///
/// Prefers the structured `questions` view: it carries the short header, the
/// per-option `description`, and the position within a multi-question request —
/// none of which the flat `question` / `options` pair can express, which is why
/// this overlay showed a bare label where a messaging channel showed
/// `label — description`.
///
/// Falls back to the flat pair whenever the structured view is absent or its
/// cursor is out of range (a frame that raced a completion), so the overlay
/// renders *something* rather than an empty box. The fallback reports
/// `multi_select` / `secret` as false because the flat pair cannot express
/// either — narrowing, never widening: the worst it costs is an unmasked
/// buffer on a core too old to have had `secret` at all.
fn render_ask_user(
    question: &str,
    options: &[String],
    questions: &[AskUserQuestion],
    answered: usize,
) -> AskDialogView {
    let Some(current) = questions.get(answered) else {
        return AskDialogView {
            question: question.to_string(),
            options: options.to_vec(),
            multi_select: false,
            secret: false,
        };
    };
    let position = if questions.len() > 1 {
        format!("({}/{}) ", answered + 1, questions.len())
    } else {
        String::new()
    };
    let header = current
        .header
        .as_deref()
        .map(|h| format!("[{h}] "))
        .unwrap_or_default();
    let hint = if current.multi_select {
        "\n(pick one or more — reply with comma-separated numbers)"
    } else {
        ""
    };
    let labels = current
        .options
        .iter()
        .map(|o| match o.description.as_deref() {
            Some(d) if !d.trim().is_empty() => format!("{} — {d}", o.label),
            _ => o.label.clone(),
        })
        .collect();
    AskDialogView {
        question: format!("{position}{header}{}{hint}", current.prompt),
        options: labels,
        multi_select: current.multi_select,
        secret: current.secret,
    }
}

impl AppState {
    /// Request application quit. Sets `should_quit` flag.
    pub const fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Record which session `run_id` belongs to, learned from that run's own
    /// `RunAccepted`. Recorded for EVERY run, own or foreign: a run's home
    /// session cannot change, but [`AppState::session_key`] can (a
    /// `/session` switch), so `frame_belongs_here` re-derives "does this
    /// belong on THIS screen" against the current key every time instead of
    /// baking in a yes/no at learn time — the same run correctly resumes
    /// once the screen switches back to its actual session, instead of
    /// staying dropped forever because it was foreign a moment ago.
    ///
    /// Idempotent (a repeated id is not pushed twice, so a resend cannot
    /// burn through the FIFO bound early) and bounded at
    /// [`RUN_SESSION_CAP`] (see its doc for why eviction is safe).
    fn mark_run_session(&mut self, run_id: String, session_key: String) {
        if self.run_sessions.iter().any(|(id, _)| *id == run_id) {
            return;
        }
        if self.run_sessions.len() >= RUN_SESSION_CAP {
            self.run_sessions.pop_front();
        }
        self.run_sessions.push_back((run_id, session_key));
    }

    /// Whether a frame naming `run_id` belongs on THIS screen right now.
    ///
    /// Only a run id whose recorded home session does not match
    /// [`AppState::session_key`] *at this moment* is dropped. An id this
    /// screen has never learned about is kept: "I cannot tell" must not
    /// become "not mine", or a run whose `RunAccepted` raced past this frame
    /// (or a core too old to send one at all) goes silently missing from its
    /// own transcript.
    #[must_use]
    pub fn frame_belongs_here(&self, run_id: &str) -> bool {
        match self.run_sessions.iter().find(|(id, _)| id == run_id) {
            Some((_, session_key)) => *session_key == self.session_key,
            None => true,
        }
    }

    /// Apply one frame belonging to a side question to the `/btw` overlay.
    ///
    /// Every arm ends in the overlay or is deliberately ignored; none of them
    /// touches `messages`, `current_run` or the run-scoped status fields. That
    /// is the property Step 5 of this task asserts on a real machine and
    /// `no_side_question_frame_reaches_the_main_transcript` asserts here: a
    /// side question is answered on a session the user is not looking at, so
    /// nothing about it may appear in the conversation they ARE looking at.
    ///
    /// Ignoring the rest is not laziness — a side question runs read-only on a
    /// derived session, so its context gauge, model-fallback notice and retry
    /// ladder describe that session, not this screen's. Applying them would be
    /// the same defect the cross-session guard exists to prevent, arriving by
    /// a route that bypasses it.
    fn apply_btw_frame(&mut self, event: StreamEvent) -> Action {
        use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};

        // Every application below names the run, never just "the active
        // question". `accepts_frame` says the frame is the overlay's; it does
        // NOT say it belongs to what is on screen, because a claim outlives
        // its exchange and a second side question can be asked while the first
        // is still answering. See `BtwOverlay::for_active_run`.
        let run_id = event.run_id().to_string();

        match event {
            StreamEvent::RunAccepted {
                run_id,
                session_key,
                ..
            } => {
                // Recorded even though this frame is being intercepted: if the
                // overlay's claim is ever evicted, `frame_belongs_here` is the
                // fallback, and it can only answer for a run it has learned
                // about. The side key is not this screen's key, so it answers
                // "drop" — which is the safe direction for a stray side frame.
                self.mark_run_session(run_id, session_key);
                Action::None
            }
            StreamEvent::ResponseChunk { content, .. } => {
                self.btw.push_delta(&run_id, &content);
                Action::None
            }
            StreamEvent::AgentTrace { event, .. } => {
                match event {
                    // The turn's full text; the deltas above are its prefix.
                    AgentTraceEvent::TextEmitted {
                        stream: AgentTraceTextKind::Final,
                        text,
                        ..
                    } => self.btw.push_final(&run_id, &text),
                    AgentTraceEvent::ToolCallStarted { call, .. } => {
                        self.btw.note_tool(&run_id, Some(call.tool_name));
                    }
                    AgentTraceEvent::ToolCallCompleted { .. } => self.btw.note_tool(&run_id, None),
                    _ => {}
                }
                Action::None
            }
            StreamEvent::RunComplete { summary, .. } => {
                self.btw
                    .finish_active(&run_id, summary.final_response.as_deref());
                Action::None
            }
            StreamEvent::RunError { error, .. } => {
                self.btw.fail_active(&run_id, error);
                Action::None
            }
            _ => Action::None,
        }
    }

    // -- Gateway event handling -----------------------------------------

    /// Handle a `StreamEvent` from the gateway. Returns an Action if the event
    /// should trigger further side effects (e.g. scrolling to bottom).
    pub fn handle_gateway_event(&mut self, event: StreamEvent) -> Action {
        // Cross-session guard, structural rather than one early-return per
        // arm: see `run_scoped_id` for what is exempt and why. Everything
        // else — whether it appends visible transcript text (`ResponseChunk`,
        // `AgentTrace`, tool rows, reasoning, the system-message arms) or
        // repaints run-scoped status (`ModelResolved`, `ContextGauge`) — is
        // "another run's state silently applied to this screen," the same
        // defect either way, so both kinds are dropped here before a single
        // arm below runs.
        // Side-question intercept, and it MUST run before the guard below.
        //
        // A `/btw` run executes on a *derived* session, so its frames are
        // cross-session by construction and `frame_belongs_here` correctly
        // drops every one of them — correct for the transcript, useless for
        // the person who asked. Claiming them here routes them to the overlay
        // instead, and returning is the other half of the same requirement:
        // falling through to the arms below is exactly how a side question
        // would end up IN the main transcript.
        //
        // This does not weaken the guard. The guard protects against every
        // other foreign run; this is one narrow branch for run ids the
        // overlay itself asked for.
        if self.btw.accepts_frame(run_scoped_id(&event)) {
            return self.apply_btw_frame(event);
        }

        if let Some(run_id) = run_scoped_id(&event) {
            if !self.frame_belongs_here(run_id) {
                return Action::None;
            }
        }

        match event {
            StreamEvent::RunAccepted {
                run_id,
                session_key,
                ..
            } => {
                // Learn this run's home session unconditionally — own or
                // foreign — so `frame_belongs_here` can answer for it later
                // no matter how many times `self.session_key` changes
                // afterward (see `mark_run_session`'s doc). This frame
                // itself always reaches here un-guarded: nothing could have
                // proven it foreign before its own `RunAccepted`, which is
                // what teaches it.
                self.mark_run_session(run_id.clone(), session_key.clone());
                if session_key != self.session_key {
                    // Someone else's run: a background/cron/delegated-
                    // subagent run, or another window's conversation. Every
                    // later frame naming it is caught by the guard above and
                    // dropped instead of leaking into this transcript.
                    return Action::None;
                }
                self.current_run = Some(run_id);
                self.run_started_at = Some(Instant::now());
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.turn_streamed_len = 0;
                self.run_rendered_assistant_text = false;
                self.is_connected = true;
                Action::None
            }

            // The gateway does emit this frame (`busy_queue::deliver_with_
            // ticket`'s `report(ahead)`) — Panel renders it as a queued
            // phase. This screen has no equivalent "waiting" presentation
            // yet, so the frame is a no-op here rather than unreachable.
            // Exhaustiveness still requires an arm for the variant, per
            // `run_scoped_id`'s doc above.
            StreamEvent::RunQueued { .. } => Action::None,

            StreamEvent::AgentTrace { event, .. } => {
                self.current_run_uses_agent_trace = true;
                self.apply_agent_trace_event(&event)
            }

            StreamEvent::Reasoning { content, .. } => {
                self.ensure_assistant_message();
                if let ChatMessage::Assistant { reasoning, .. } = self.current_assistant_mut() {
                    match reasoning {
                        Some(existing) => existing.push_str(&content),
                        None => *reasoning = Some(content),
                    }
                }
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ToolStart {
                tool_name,
                tool_id,
                params,
                ..
            } => {
                if self.current_run_uses_agent_trace {
                    return Action::None;
                }
                if matches!(self.tool_progress_mode, ToolProgressMode::Off) {
                    return Action::None;
                }
                self.start_tool_execution(
                    tool_id,
                    tool_name,
                    summarize_tool_input(&params, AgentTracePresentationPreset::TuiDebug.options()),
                );
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ToolUpdate {
                tool_id, progress, ..
            } => {
                // /tools off|new — suppress mid-execution progress updates entirely.
                // /tools all|verbose — surface them.
                match self.tool_progress_mode {
                    ToolProgressMode::Off | ToolProgressMode::New => return Action::None,
                    ToolProgressMode::All | ToolProgressMode::Verbose => {}
                }
                if let Some(tool) = self.find_tool_mut(&tool_id) {
                    tool.progress = Some(progress);
                }
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ToolEnd {
                tool_id,
                result,
                duration_ms,
                ..
            } => {
                // Deliberately NOT gated on `current_run_uses_agent_trace`, and
                // deliberately asymmetric with `ToolStart` above.
                //
                // `ToolEnd` rides the authoritative stream while
                // `AgentTrace{ToolCallCompleted}` rides the lossy mirror, so
                // letting this one through is what settles a row whose trace
                // frame was dropped. It is safe to double-apply because
                // `finish_tool_execution` only ever moves a row to a terminal
                // state and no-ops on an unknown id.
                //
                // `ToolStart` stays gated: `start_tool_execution` RESETS a row
                // to Running and clears its duration/error, so an out-of-order
                // arrival would un-complete a finished tool — and the trace
                // mirror's `summarize_tool_input` params render better than the
                // raw ones here.
                if matches!(self.tool_progress_mode, ToolProgressMode::Off) {
                    return Action::None;
                }
                let result = if result.success {
                    AgentTraceToolResult::Success {
                        output: serde_json::json!(result.output),
                    }
                } else {
                    AgentTraceToolResult::Error {
                        error: result.error.unwrap_or_default(),
                        retryable: false,
                    }
                };
                self.finish_tool_execution(&tool_id, &result, duration_ms);
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ResponseChunk { content, .. } => {
                // Deliberately NOT gated on `current_run_uses_agent_trace`:
                // `TextEmitted{Final}` arrives once per turn, so gating here
                // would kill the live typewriter. The de-dup happens on the
                // other side — see `turn_streamed_len`.
                self.turn_streamed_len += content.len();
                self.append_assistant_content(&content);
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunComplete {
                summary,
                total_duration_ms,
                ..
            } => {
                self.current_run = None;
                self.run_started_at = None;
                self.dismiss_pending_approval();
                self.last_run_duration = Some(Duration::from_millis(total_duration_ms));
                if !self.current_run_trace_summary_applied {
                    self.update_token_usage(&summary);
                }
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.turn_streamed_len = 0;
                // End-of-stream reconciliation against the authoritative
                // terminal record — the live rows came off the lossy
                // `agent_trace` mirror. Order matters: fill from the summary
                // first, then settle whatever it did not mention.
                self.reconcile_tools_from_summary(&summary.tool_summaries, &summary.errors);
                self.settle_orphan_tools();
                // The terminal answer, when this run produced no other copy of
                // it. On an ordinary streamed turn the text is already on
                // screen (twice on the wire, de-duped by `turn_streamed_len`)
                // and `final_response` is a duplicate, so the flag is what
                // keeps this from doubling every reply. On a run the gateway
                // SERVED rather than dispatched — `/btw promote` is the first —
                // this frame is the only carrier the answer ever gets, and
                // without this the surface that owns the `p` key showed a
                // spinner and then silence for both of the outcomes that are
                // not errors.
                if !self.run_rendered_assistant_text {
                    if let Some(text) = summary.final_response.as_deref() {
                        self.append_assistant_content(text.trim_end());
                    }
                }
                self.mark_current_assistant_complete();

                // Surface non-clean terminations (a hit cap / exhausted budget)
                // so a truncated answer doesn't read as a clean finish.
                // `terminate_detail` carries the granular cap inside the budget
                // umbrella; fall back to the reason token when absent.
                if let Some(reason) = summary.terminate_reason.as_deref() {
                    if reason != "completed" {
                        let raw = summary.terminate_detail.as_deref().unwrap_or(reason);
                        let label = match raw {
                            "hit_max_iterations" => "hit max iterations",
                            "context_budget_exhausted" => "context budget exhausted",
                            "max_output_tokens_exhausted" => "max output tokens reached",
                            "budget_exhausted_partial_result" => {
                                "budget exhausted (partial result)"
                            }
                            other => other,
                        };
                        self.add_system_message(format!("Run stopped: {label}"));
                    }
                }

                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunError { error, .. } => {
                self.current_run = None;
                self.run_started_at = None;
                self.dismiss_pending_approval();
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.turn_streamed_len = 0;
                // No summary on this path, so there is nothing to reconcile
                // against — but the run is over, so a spinning row is a lie
                // either way.
                self.settle_orphan_tools();
                self.mark_current_assistant_complete();

                self.add_system_message(format!("Error: {error}"));
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::AskUser {
                session_key,
                question,
                options,
                questions,
                answered,
                ..
            } => {
                // The structured view when core sends one, the flat projection
                // otherwise. Indices are identical either way, which is what
                // lets the overlay keep answering with a bare 1-based number.
                let view = render_ask_user(&question, &options, &questions, answered);
                self.show_dialog(session_key, view);
                Action::None
            }

            // The question is over — retire the card. Without this the overlay
            // holds focus and keeps claiming the agent is waiting for up to the
            // 600 s clarification timeout, and an answer typed into it is
            // silently discarded by a registry that has already forgotten the
            // entry. Only the card for THIS session is retired: a frame for
            // another session must not close the one the user is looking at.
            //
            // The ordinary path is silent without a special case for it:
            // answering already ran `close_overlay`, so by the time the
            // `resolved` frame lands there is no dialog and `mine` is false.
            // What is left is exactly the set worth telling the user about —
            // expired, cancelled with its run, superseded, or answered in
            // another window.
            StreamEvent::ClarificationEnded {
                session_key,
                outcome,
            } => {
                let mine = self
                    .dialog
                    .as_ref()
                    .is_some_and(|d| d.session_key == session_key);
                if !mine {
                    return Action::None;
                }
                self.close_overlay();
                // Say WHICH ending it was rather than letting the card vanish:
                // "you answered" and "it expired while you were reading" leave
                // the user in very different positions. The outcome word is
                // core's, printed verbatim — this client does not own that
                // vocabulary and must not paraphrase it.
                self.add_system_message(format!("The agent's question ended ({outcome})."));
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ReasoningBlock { content, .. } => {
                if self.current_run_uses_agent_trace {
                    return Action::None;
                }
                // Treated same as Reasoning — append to reasoning buffer
                self.append_reasoning_entry(content);
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::UncertaintySignal {
                uncertainty,
                suggested_action,
                ..
            } => {
                let msg = format!(
                    "Uncertainty: {} ({})",
                    uncertainty,
                    suggested_action.description()
                );
                self.add_system_message(msg);
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunRetrying {
                provider,
                attempt,
                max_attempts,
                reason,
                ..
            } => {
                // Surface transient provider failures instead of leaving the
                // thinking indicator spinning silently through the retry
                // ladder (mirrors the Panel's stream.run_retrying notice).
                self.add_system_message(format!(
                    "Provider {provider} unreachable, retrying ({attempt}/{max_attempts}): {reason}"
                ));
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ModelResolved { model_info, .. } => {
                // Only fallback selections are worth a system line — the
                // happy path would announce the model on every run.
                if model_info.is_fallback {
                    let requested = model_info
                        .original_model
                        .as_deref()
                        .filter(|orig| *orig != model_info.model);
                    let line = match requested {
                        Some(orig) => format!(
                            "Model fallback: {orig} → {} (via {})",
                            model_info.model, model_info.provider
                        ),
                        None => format!(
                            "Model fallback: {} (via {})",
                            model_info.model, model_info.provider
                        ),
                    };
                    self.add_system_message(line);
                    return Action::ScrollToBottomIfAutoScroll;
                }
                Action::None
            }

            StreamEvent::ContextGauge {
                context_tokens,
                context_window,
                ..
            } => {
                // Live context-window occupancy (mirrors the Panel gauge). Only
                // the numerator/denominator are stored; the event's own
                // `total_tokens` is deliberately ignored — RunComplete's summary
                // already owns the running token tally, and adding both would
                // double-count. Skip windowless frames so the bar never divides
                // by zero and keeps the last good reading.
                if context_window > 0 {
                    self.context_gauge = Some((context_tokens, context_window));
                }
                Action::None
            }
        }
    }
}
