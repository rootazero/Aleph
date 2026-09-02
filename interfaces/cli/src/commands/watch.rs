//! `aleph watch` — live activity board across all sessions.
//!
//! `ClawTeam` ships a Rich `Live` team board (`clawteam/board/renderer.py`)
//! that repaints collector state on a polling interval. Aleph's gateway
//! already pushes every `stream.*` frame to every authenticated WebSocket
//! connection (`SubscriptionManager` default = receive-all), so the CLI can do
//! better than poll: render the event stream itself as an append-only
//! activity feed (`kubectl get events -w` style) — one timestamped line per
//! significant run event, across *all* sessions and channels at once.
//!
//! Pure I/O (R4): every line is a straight rendering of a protocol event;
//! the only client-side state is a per-run tally for the closing summary.

use std::collections::HashMap;
use std::time::Instant;

use aleph_protocol::terminate::{self, TerminateSeverity, UiLocale};
use aleph_protocol::{AgentTraceEvent, RunSummary, StreamEvent};

use crate::output::exec_echo;
use crate::output::theme::{paint, Style};
use aleph_client::{AlephClient, CliConfig, CliResult, ClientEvent};

/// Follow live agent activity until Ctrl-C or server disconnect.
pub async fn run(
    server_url: &str,
    session_filter: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, mut events) = AlephClient::connect(server_url, config).await?;

    if !json {
        // Banner on stderr: stdout stays pure feed data.
        eprintln!(
            "{}",
            paint(
                Style::Muted,
                &format!("watching live agent activity on {server_url} (Ctrl-C to stop)"),
            )
        );
        if let Some(s) = session_filter {
            eprintln!("{}", paint(Style::Muted, &format!("session filter: {s}")));
        }
    }

    let mut board = Board::new();
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            _ = &mut ctrl_c => break,
            ev = events.recv() => match ev {
                // `aleph watch` is the intentional firehose consumer of the
                // `stream.*` plane (see `run_follow`'s module doc); a topic
                // frame (`events.subscribe`) is a different surface this
                // command does not render. Explicit, not `_ =>` — the next
                // `ClientEvent` variant added must make whoever extends this
                // loop decide what it means here, not silently pass through.
                Some(ClientEvent::Topic { .. }) => continue,
                Some(ClientEvent::Stream(event)) => {
                    let event = *event;
                    if !board.admits(&event, session_filter) {
                        continue;
                    }
                    if json {
                        if let Ok(line) = serde_json::to_string(&event) {
                            println!("{line}");
                        }
                        board.observe(&event);
                        continue;
                    }
                    let lines = board.observe(&event);
                    for body in lines {
                        println!("{}{body}", feed_prefix(event.run_id()));
                    }
                }
                None => {
                    if !json {
                        eprintln!("{}", paint(Style::Warning, "server closed the connection"));
                    }
                    break;
                }
            },
        }
    }

    if !json {
        eprintln!();
        eprintln!("{}", board.closing_summary());
    }

    client.close().await?;
    Ok(())
}

/// `HH:MM:SS run-id  ` line prefix; both columns muted/cyan so the event
/// body keeps visual priority.
fn feed_prefix(run_id: &str) -> String {
    let ts = chrono::Local::now().format("%H:%M:%S").to_string();
    format!(
        "{} {}",
        paint(Style::Muted, &ts),
        paint(Style::Info, &format!("{:<12}", short_run_id(run_id))),
    )
}

/// Compact a run id for the feed column: drop the conventional `run-`
/// prefix and keep the first 8 chars of the remainder (UUID head).
fn short_run_id(run_id: &str) -> String {
    let core = run_id.strip_prefix("run-").unwrap_or(run_id);
    core.chars().take(8).collect()
}

/// Per-run tally + event-to-feed-line rendering.
struct Board {
    started: Instant,
    runs: HashMap<String, RunEntry>,
    completed: usize,
    failed: usize,
    verbose: bool,
}

#[derive(Default)]
struct RunEntry {
    session_key: Option<String>,
    /// `AgentTrace` carries richer tool info than the coarse `ToolStart`
    /// mirror; once seen for a run, the coarse events are suppressed
    /// (same dedup rule as `run_follow`).
    trace_seen: bool,
    settled: bool,
}

impl Board {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            runs: HashMap::new(),
            completed: 0,
            failed: 0,
            verbose: std::env::var("ALEPH_VERBOSE").is_ok(),
        }
    }

    /// Session filter: `RunAccepted` binds run → session; later events are
    /// admitted iff their run's recorded session matches. Runs already
    /// in-flight when the watch started have no binding and are hidden
    /// under a filter (their session is unknowable client-side).
    fn admits(&self, event: &StreamEvent, filter: Option<&str>) -> bool {
        let Some(filter) = filter else { return true };
        match event {
            StreamEvent::RunAccepted { session_key, .. } => session_key == filter,
            other => self
                .runs
                .get(other.run_id())
                .and_then(|e| e.session_key.as_deref())
                .is_some_and(|s| s == filter),
        }
    }

    /// Update tallies and return the feed lines (already styled, no prefix)
    /// this event produces. Most events produce zero or one line.
    fn observe(&mut self, event: &StreamEvent) -> Vec<String> {
        match event {
            StreamEvent::RunAccepted { session_key, .. } => {
                let entry = self.entry(event.run_id());
                entry.session_key = Some(session_key.clone());
                vec![format!(
                    "{} {}",
                    paint(Style::Info, &format!("{} run accepted", glyph_play())),
                    paint(Style::Muted, &format!("session {session_key}")),
                )]
            }
            StreamEvent::AgentTrace { event: trace, .. } => {
                self.entry(event.run_id()).trace_seen = true;
                match trace {
                    AgentTraceEvent::ToolCallStarted { call, .. } => {
                        vec![exec_echo::render_tool_start(&call.tool_name, &call.input)]
                    }
                    AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
                        exec_echo::render_tool_end(
                            &call.tool_name,
                            result,
                            call.duration_ms,
                            self.verbose,
                        )
                        .into_iter()
                        .collect()
                    }
                    _ => vec![],
                }
            }
            StreamEvent::ToolStart {
                tool_name, params, ..
            } => {
                if self.entry(event.run_id()).trace_seen {
                    vec![]
                } else {
                    vec![exec_echo::render_tool_start(tool_name, params)]
                }
            }
            StreamEvent::RunRetrying {
                provider,
                attempt,
                max_attempts,
                reason,
                ..
            } => vec![exec_echo::render_retry_notice(
                provider,
                *attempt,
                *max_attempts,
                reason,
            )],
            StreamEvent::ModelResolved { model_info, .. } if model_info.is_fallback => {
                vec![exec_echo::render_fallback_notice(
                    &model_info.model,
                    &model_info.provider,
                    model_info.original_model.as_deref(),
                )]
            }
            StreamEvent::RunComplete {
                summary,
                total_duration_ms,
                ..
            } => {
                self.settle(event.run_id(), true);
                vec![render_settled_line(
                    summary,
                    *total_duration_ms,
                    UiLocale::from_env(),
                )]
            }
            StreamEvent::RunError { error, .. } => {
                self.settle(event.run_id(), false);
                let msg: String = error.replace('\n', " ").chars().take(160).collect();
                vec![paint(Style::Error, &format!("{} {msg}", glyph_fail()))]
            }
            // Content/reasoning stream is deliberately not mirrored — the
            // board is an activity feed, not a transcript.
            _ => vec![],
        }
    }

    fn entry(&mut self, run_id: &str) -> &mut RunEntry {
        self.runs.entry(run_id.to_string()).or_default()
    }

    fn settle(&mut self, run_id: &str, ok: bool) {
        let entry = self.entry(run_id);
        if entry.settled {
            return;
        }
        entry.settled = true;
        if ok {
            self.completed += 1;
        } else {
            self.failed += 1;
        }
    }

    fn closing_summary(&self) -> String {
        let elapsed = exec_echo::format_duration(self.started.elapsed().as_millis() as u64);
        let active = self.runs.values().filter(|e| !e.settled).count();
        paint(
            Style::Muted,
            &format!(
                "observed {} runs · {} completed · {} failed · {} still active · watched {}",
                self.runs.len(),
                self.completed,
                self.failed,
                active,
                elapsed,
            ),
        )
    }
}

/// One-line run receipt for the feed: status badge + the same stats the
/// `run_follow` footer shows, joined inline.
///
/// This line used to print the raw wire token at a person — `hit_max_iterations`
/// on the board, with nothing to say which of the two halts that even was — and
/// painted a crashed run with the same warning glyph as a capped one. Both the
/// words and the crash/cap split now come from [`aleph_protocol::terminate`],
/// the table `aleph exec` and the TUI read; `locale` is a value rather than an
/// environment read so this stays pure and its test stays independent of the
/// developer's shell.
fn render_settled_line(
    summary: &RunSummary,
    total_duration_ms: u64,
    locale: UiLocale,
) -> String {
    let token = terminate::effective_token(
        summary.terminate_reason.as_deref(),
        summary.terminate_detail.as_deref(),
    );
    let (mark, style) = match token.map_or(TerminateSeverity::Clean, terminate::severity) {
        TerminateSeverity::Clean => (glyph_ok(), Style::Success),
        TerminateSeverity::Capped => (glyph_warn(), Style::Warning),
        TerminateSeverity::Failed => (glyph_fail(), Style::Error),
    };
    let label = terminate::label(token.unwrap_or(terminate::CLEAN_TOKEN), locale);
    let mut stats = vec![paint(style, &format!("{mark} {label}"))];
    if summary.tool_calls > 0 {
        stats.push(format!("{} tools", summary.tool_calls));
    }
    if summary.total_tokens > 0 {
        stats.push(format!(
            "{} tokens",
            exec_echo::human_count(summary.total_tokens)
        ));
    }
    let dur_ms = if total_duration_ms > 0 {
        Some(total_duration_ms)
    } else {
        summary.duration_ms
    };
    if let Some(ms) = dur_ms {
        stats.push(exec_echo::format_duration(ms));
    }
    if let Some(cost) = summary.estimated_cost_usd {
        if cost > 0.0 {
            stats.push(format!("${cost:.4}"));
        }
    }
    let sep = paint(Style::Muted, " · ");
    stats.join(&sep)
}

fn glyph_play() -> &'static str {
    if crate::output::icon::use_unicode() {
        "▶"
    } else {
        "[>]"
    }
}

fn glyph_ok() -> &'static str {
    if crate::output::icon::use_unicode() {
        "✓"
    } else {
        "[ok]"
    }
}

fn glyph_warn() -> &'static str {
    if crate::output::icon::use_unicode() {
        "⚠"
    } else {
        "[!]"
    }
}

fn glyph_fail() -> &'static str {
    if crate::output::icon::use_unicode() {
        "✗"
    } else {
        "[x]"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted(run: &str, session: &str) -> StreamEvent {
        StreamEvent::RunAccepted {
            run_id: run.to_string(),
            session_key: session.to_string(),
            accepted_at: "2026-06-10T00:00:00Z".to_string(),
        }
    }

    fn complete(run: &str) -> StreamEvent {
        StreamEvent::RunComplete {
            run_id: run.to_string(),
            seq: 9,
            summary: RunSummary {
                total_tokens: 4500,
                tool_calls: 3,
                loops: 2,
                ..Default::default()
            },
            total_duration_ms: 12_300,
        }
    }

    #[test]
    fn short_run_id_strips_prefix_and_caps_length() {
        assert_eq!(short_run_id("run-0123456789abcdef"), "01234567");
        assert_eq!(short_run_id("bare"), "bare");
    }

    #[test]
    fn session_filter_binds_on_accept_and_hides_unknown_runs() {
        let mut board = Board::new();
        let accept = accepted("run-a", "tg:42");
        assert!(board.admits(&accept, Some("tg:42")));
        assert!(!board.admits(&accept, Some("other")));
        board.observe(&accept);

        // Later event for the bound run passes the matching filter only.
        let done = complete("run-a");
        assert!(board.admits(&done, Some("tg:42")));
        assert!(!board.admits(&done, Some("other")));

        // A run never bound (started before the watch) is hidden under a
        // filter but visible without one.
        let stranger = complete("run-b");
        assert!(!board.admits(&stranger, Some("tg:42")));
        assert!(board.admits(&stranger, None));
    }

    #[test]
    fn observe_tallies_settled_runs_once() {
        let mut board = Board::new();
        board.observe(&accepted("run-a", "s"));
        board.observe(&complete("run-a"));
        board.observe(&complete("run-a")); // duplicate receipt
        assert_eq!(board.completed, 1);
        let summary = board.closing_summary();
        assert!(summary.contains("1 completed"));
        assert!(summary.contains("0 failed"));
    }

    #[test]
    fn settled_line_carries_stats_inline() {
        let StreamEvent::RunComplete {
            summary,
            total_duration_ms,
            ..
        } = complete("run-a")
        else {
            unreachable!()
        };
        let line = render_settled_line(&summary, total_duration_ms, UiLocale::En);
        assert!(line.contains("completed"));
        assert!(line.contains("3 tools"));
        assert!(line.contains("4.5k tokens"));
        assert!(line.contains("12.3s"));
        assert!(!line.contains('\n'));
    }

    /// The board printed the wire token verbatim at a person, and gave a
    /// crashed run the same warning glyph as a capped one.
    #[test]
    fn settled_line_uses_words_and_separates_a_crash_from_a_cap() {
        let halted = |reason: &str, detail: Option<&str>| RunSummary {
            terminate_reason: Some(reason.to_string()),
            terminate_detail: detail.map(str::to_string),
            ..Default::default()
        };

        let capped = render_settled_line(&halted("hit_max_iterations", None), 0, UiLocale::En);
        assert!(capped.contains("hit max iterations"), "{capped}");
        assert!(!capped.contains("hit_max_iterations"), "{capped}");
        assert!(capped.contains(glyph_warn()), "{capped}");

        let died = render_settled_line(&halted("failed", None), 0, UiLocale::En);
        assert!(died.contains(glyph_fail()), "a crash is not a cap: {died}");

        // Same precedence as every other surface: the detail names the cap
        // hiding under the umbrella token.
        let umbrella = render_settled_line(
            &halted(
                "budget_exhausted_partial_result",
                Some("context_budget_exhausted"),
            ),
            0,
            UiLocale::Zh,
        );
        assert!(umbrella.contains("上下文预算耗尽"), "{umbrella}");
    }
}
