//! Shared "follow a run" event loop for non-interactive commands.
//!
//! `ask` and `chat-control send --stream` both dispatch a run over the same
//! WebSocket and need identical presentation afterwards: live tool activity
//! on stderr, an incrementally rendered Markdown body on stdout, provider
//! retry notices, and the run-summary receipt footer. This module is the
//! single source for that loop — it was previously inlined in `ask.rs`,
//! while `chat-control send --stream` forwarded the flag to the server and
//! then dropped the event stream entirely.
//!
//! Three behaviours improve on the old inline loop:
//!
//! 1. **Streaming body** — `ResponseChunk`s render incrementally through
//!    [`MarkdownStream`] instead of buffering until end-of-run, so the user
//!    reads the answer as it is produced (hermes-agent stable-prefix model).
//! 2. **Waiting feedback** — a [`Spinner`] covers the dispatch → first-token
//!    gap (it was already built for `connect`/`open` but never used here).
//! 3. **Receipt fidelity** — a final `ResponseChunk{is_final}` no longer
//!    aborts the loop outright; the loop waits a short grace period for the
//!    `RunComplete` receipt so the stats footer (tokens/duration/cost)
//!    actually renders on the common path. Provider retries
//!    (`stream.run_retrying`) surface as an amber status line instead of
//!    minutes of silence.

use std::time::Duration;

use aleph_protocol::{AgentTraceEvent, StreamEvent};
use tokio::sync::mpsc;

use crate::output::{exec_echo, markdown, stream_md::MarkdownStream, Spinner};

/// How long to wait for the `RunComplete` receipt after the final response
/// chunk. The drain emits it immediately after the run settles, so this only
/// triggers against a dead server.
const FINAL_GRACE: Duration = Duration::from_secs(3);

pub struct FollowOptions {
    /// Emit every raw protocol event as one JSONL line on stdout and
    /// suppress all human rendering.
    pub json: bool,
    /// Surface reasoning previews and tool result previews.
    pub verbose: bool,
    /// Pin the loop to one run. The gateway broadcasts every `stream.*`
    /// frame to every authenticated connection (`SubscriptionManager` default
    /// = receive-all), so a concurrent cron/channel run would otherwise
    /// interleave its tool lines and body chunks into this command's
    /// output — and worse, its `RunComplete` would terminate the loop
    /// early. `None` follows everything (legacy behaviour; `aleph watch`
    /// is the intentional firehose consumer).
    pub run_id: Option<String>,
}

pub struct FollowOutcome {
    /// The resolved final answer: streamed text when present, else the
    /// authoritative `RunSummary.final_response` fallback.
    pub final_text: String,
}

/// Drain the event stream for one run, rendering as it goes. Returns once
/// the run settles (`RunComplete` / `RunError`), the channel closes, or the
/// post-final grace period elapses.
pub async fn follow_run(
    events: &mut mpsc::Receiver<StreamEvent>,
    opts: &FollowOptions,
) -> FollowOutcome {
    let mut streamed_raw = String::new();
    let mut fallback: Option<String> = None;
    let mut tool_count = 0usize;
    let mut agent_trace_seen = false;
    let mut footer_rendered = false;
    let mut saw_final_chunk = false;
    let mut printed_body = false;
    let mut md = MarkdownStream::new();

    // Cover the "dispatch → first model token" black gap. The spinner is a
    // no-op off-TTY and is stopped before the first rendered line; the
    // immediate RunAccepted ack deliberately keeps it spinning.
    let mut spinner = if opts.json {
        None
    } else {
        Some(Spinner::start("thinking…"))
    };

    loop {
        let event = if saw_final_chunk {
            // Body complete — wait briefly for the RunComplete receipt
            // (single-source emission) without hanging on a dead server.
            match tokio::time::timeout(FINAL_GRACE, events.recv()).await {
                Ok(Some(ev)) => ev,
                Ok(None) | Err(_) => break,
            }
        } else {
            match events.recv().await {
                Some(ev) => ev,
                None => break,
            }
        };

        // Foreign-run events (concurrent cron / channel / Panel runs on the
        // same broadcast bus) are not ours to render or terminate on.
        if let Some(expected) = opts.run_id.as_deref() {
            if event.run_id() != expected {
                continue;
            }
        }

        // JSON mode: serialize every event before any presentation logic so
        // consumers see the raw protocol stream.
        if opts.json {
            if let Ok(line) = serde_json::to_string(&event) {
                println!("{line}");
            }
        }

        // First renderable signal ends the waiting phase.
        if !matches!(event, StreamEvent::RunAccepted { .. }) {
            if let Some(s) = spinner.take() {
                s.stop().await;
            }
        }

        match event {
            // Rich path: the agent-loop trace carries tool args, per-call
            // duration, the scratchpad checklist, and the terminate cause.
            StreamEvent::AgentTrace { event, .. } => {
                agent_trace_seen = true;
                if opts.json {
                    continue;
                }
                match event {
                    AgentTraceEvent::ToolCallStarted { call, .. } => {
                        tool_count += 1;
                        eprintln!(
                            "{}",
                            exec_echo::render_tool_start(&call.tool_name, &call.input)
                        );
                    }
                    AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
                        if let Some(line) = exec_echo::render_tool_end(
                            &call.tool_name,
                            &result,
                            call.duration_ms,
                            opts.verbose,
                        ) {
                            eprintln!("{line}");
                        }
                    }
                    AgentTraceEvent::ToolSummary { summary, .. } if opts.verbose => {
                        if let Some(line) = exec_echo::render_tool_summary(&summary) {
                            eprintln!("{line}");
                        }
                    }
                    // Recovery / watchdog moments — always shown (rare,
                    // high-signal): they explain *why* the run changed course.
                    AgentTraceEvent::ReactiveCompactionAttempted {
                        token_gap,
                        succeeded,
                    } => {
                        eprintln!(
                            "{}",
                            exec_echo::render_compaction_notice(token_gap, succeeded)
                        );
                    }
                    AgentTraceEvent::VerifierVeto { reason, .. } => {
                        eprintln!("{}", exec_echo::render_veto_notice(&reason));
                    }
                    _ => {}
                }
            }
            StreamEvent::ResponseChunk {
                content, is_final, ..
            } => {
                streamed_raw.push_str(&content);
                if !opts.json {
                    if let Some(block) = md.push(&content) {
                        emit_body_block(&block, &mut printed_body);
                    }
                }
                if is_final {
                    saw_final_chunk = true;
                }
            }
            // Fallback path: coarse tool events when no AgentTrace stream is
            // present (older servers / minimal emitters).
            StreamEvent::ToolStart {
                tool_name, params, ..
            } => {
                if !agent_trace_seen && !opts.json {
                    tool_count += 1;
                    eprintln!("{}", exec_echo::render_tool_start(&tool_name, &params));
                }
            }
            StreamEvent::RunRetrying {
                provider,
                attempt,
                max_attempts,
                reason,
                ..
            } => {
                if !opts.json {
                    eprintln!(
                        "{}",
                        exec_echo::render_retry_notice(&provider, attempt, max_attempts, &reason)
                    );
                }
            }
            StreamEvent::ModelResolved { model_info, .. } => {
                // Only the fallback case is signal; the happy path would be
                // noise on every run.
                if !opts.json && model_info.is_fallback {
                    eprintln!(
                        "{}",
                        exec_echo::render_fallback_notice(
                            &model_info.model,
                            &model_info.provider,
                            model_info.original_model.as_deref(),
                        )
                    );
                }
            }
            StreamEvent::RunComplete { summary, .. } => {
                // Capture the authoritative final answer for the no-stream
                // fallback and the `-o` file.
                fallback = summary.final_response.clone();
                if !opts.json {
                    // Body before receipt: flush any pending stream suffix
                    // (or the summary fallback) so the footer reads last.
                    flush_body(
                        &mut md,
                        &mut printed_body,
                        &streamed_raw,
                        fallback.as_deref(),
                    );
                    eprintln!();
                    eprintln!("{}", exec_echo::render_summary_footer(&summary));
                    footer_rendered = true;
                }
                break;
            }
            StreamEvent::RunError { error, .. } => {
                if !opts.json {
                    eprintln!("Error: {error}");
                }
                break;
            }
            StreamEvent::AskUser {
                question,
                options,
                questions,
                answered,
                ..
            } => {
                if !opts.json {
                    eprintln!(
                        "{}",
                        exec_echo::render_ask_user(&question, &options, &questions, answered)
                    );
                }
            }
            StreamEvent::UncertaintySignal { uncertainty, .. } => {
                if !opts.json {
                    if let Some(line) = exec_echo::render_uncertainty(&uncertainty) {
                        eprintln!("{line}");
                    }
                }
            }
            StreamEvent::Reasoning { content, .. } => {
                if opts.verbose && !opts.json {
                    if let Some(line) = exec_echo::render_reasoning(&content) {
                        eprintln!("{line}");
                    }
                }
            }
            StreamEvent::ReasoningBlock { content, .. }
                if opts.verbose && !agent_trace_seen && !opts.json =>
            {
                if let Some(line) = exec_echo::render_reasoning(&content) {
                    eprintln!("{line}");
                }
            }
            _ => {}
        }
    }

    if let Some(s) = spinner.take() {
        s.stop().await;
    }

    // Exits that skipped RunComplete (grace timeout, RunError, closed
    // channel) still owe the user any unflushed body text; the legacy tool
    // count stands in when no receipt footer arrived.
    if !opts.json {
        flush_body(
            &mut md,
            &mut printed_body,
            &streamed_raw,
            fallback.as_deref(),
        );
        if !footer_rendered && tool_count > 0 {
            eprintln!();
            eprintln!("({tool_count} tools used)");
        }
    }

    let final_text = exec_echo::resolve_final_text(&streamed_raw, fallback.as_deref()).to_string();
    FollowOutcome { final_text }
}

/// Print one rendered Markdown block to stdout, blank-line separated from
/// the previous one.
fn emit_body_block(rendered: &str, printed: &mut bool) {
    if *printed {
        println!();
    }
    println!("{rendered}");
    *printed = true;
}

/// Flush the unstable stream suffix; when nothing streamed at all, render
/// the summary's authoritative final answer instead (reasoning/tool-only
/// runs would otherwise print an empty body). Safe to call twice — the
/// stream drains and `printed` guards the fallback.
fn flush_body(
    md: &mut MarkdownStream,
    printed: &mut bool,
    streamed_raw: &str,
    fallback: Option<&str>,
) {
    if let Some(rest) = md.finish() {
        emit_body_block(&rest, printed);
    }
    if !*printed {
        let text = exec_echo::resolve_final_text(streamed_raw, fallback);
        if !text.trim().is_empty() {
            emit_body_block(&markdown::render(text), printed);
        }
    }
}
