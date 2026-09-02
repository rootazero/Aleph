//! `aleph-server resume <session-key>` — re-trigger an interrupted run.
//!
//! Policy: `NoLock`. This command never writes to the data directory; it reads
//! the endpoint file and the bearer token and forwards to the running server.
//!
//! Deliberately **not** `LockOrIpc`. The local half of that policy exists for
//! commands that can do their job with the lock and no server. Resuming a run
//! cannot: it means re-entering the harness with the session's provider, tools
//! and workspace. A local fallback would either silently do nothing or stand up
//! a second runtime beside the singleton, so when no server is running the
//! honest answer is to say so — which is what `forward_to_server` already does
//! when the endpoint file is missing.

use std::error::Error;

use aleph_protocol::resume::{ResumeReceipt, ResumeStatus};
use alephcore::cli::ipc_client::forward_to_server;
use alephcore::cli::policy::{run_no_lock, HttpMethod};
use alephcore::utils::paths;

/// Handle `aleph-server resume`.
pub fn handle_resume_command(session_key: String, json: bool) -> Result<(), Box<dyn Error>> {
    let data_dir = paths::get_data_dir().map_err(|e| format!("data dir: {e}"))?;
    let body = serde_json::json!({ "session_key": &session_key });

    let response: ResumeReceipt =
        run_no_lock(|| forward_to_server(&data_dir, HttpMethod::Post, "/v1/admin/resume", body))
            .map_err(|e| -> Box<dyn Error> { format!("{e:#}").into() })?;

    if json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    let named = response.session_key.as_deref().unwrap_or(&session_key);

    // Each outcome gets its own sentence. `resumed` and `no_runs` are both
    // successful exits, and an operator who cannot tell them apart will sit
    // waiting for output from a session that never had a run to resume.
    //
    // Matched **exhaustively**, with no catch-all: a new outcome must be given
    // a sentence here before this compiles. The `other => println!("{other}")`
    // arm this replaces printed a raw status word as though it were an
    // instruction, and it printed it for the three words the server had already
    // been writing for months (`delegated`, `already_resuming`,
    // `log_inconsistent`) — a wire vocabulary can grow silently, a `match`
    // cannot.
    match response.outcome() {
        ResumeStatus::Resumed => println!(
            "Resumed the interrupted run in {named}. Watch the session for output."
        ),
        ResumeStatus::AlreadyFinished => {
            println!("Nothing to resume: the newest run in {named} already finished.");
        }
        ResumeStatus::NoRuns => println!("Nothing to resume: {named} has no run history."),
        ResumeStatus::Abandoned => println!(
            "The interrupted run in {named} was abandoned (too old, or it crashed on every \
             previous resume). It will not be retried."
        ),
        ResumeStatus::NotResumed => println!(
            "Found an interrupted run in {named} but could not re-trigger it. \
             Check the server log for the reason."
        ),
        ResumeStatus::Delegated => println!(
            "The interrupted run in {named} belongs to a scheduler that runs its own \
             recovery (cron, heartbeat, or the team dispatcher). Its dangling marker was \
             closed; that scheduler will re-drive the work."
        ),
        ResumeStatus::AlreadyResuming => println!(
            "A resume for {named} is already in flight. Nothing was looked at — try again \
             once it settles."
        ),
        ResumeStatus::LogInconsistent => println!(
            "{named}'s event log contradicts itself, so its run state could not be read and \
             nothing was tried. Run `aleph doctor` — the `core/session-log` check names the \
             contradiction."
        ),
        ResumeStatus::Unavailable => println!(
            "This server has no run executor wired, so nothing can be resumed on it."
        ),
        ResumeStatus::Failed => println!(
            "Resume failed for {named}: {}",
            response.error.as_deref().unwrap_or("no reason given")
        ),
        ResumeStatus::NotFound => println!("No such session: {named}."),
        ResumeStatus::InvalidSessionKey => {
            println!("{session_key} is not a session key.");
        }
        ResumeStatus::AgentForbidden => println!(
            "Not authorized to run as agent '{}'.",
            response.agent_id.as_deref().unwrap_or("<unnamed>")
        ),
        // A word this build has never heard of. It is NOT rendered as a
        // sentence — the whole point of the closed set is that an unknown
        // status reads as "I cannot vouch for this", never as an outcome.
        ResumeStatus::Unrecognized => println!(
            "The server reported an outcome this build does not recognise ({:?}) for {named}. \
             Re-run with --json to see the whole receipt.",
            response.status
        ),
    }

    // The counters that only ever reached the JSON-RPC face before. Printed
    // only when non-zero: a line of zeroes on every successful resume trains an
    // operator to stop reading them, which is how a real one goes unnoticed.
    for entry in &response.refused {
        println!("  refused {}: {} — {}", entry.session_key, entry.reason, entry.detail);
    }
    if response.degraded > 0 {
        println!(
            "  {} run(s) came back degraded — the model was told what it lost.",
            response.degraded
        );
    }
    if response.unsnapshotted > 0 {
        println!(
            "  {} run(s) had no settings snapshot; they resume under this session's \
             current values, not the ones they crashed with.",
            response.unsnapshotted
        );
    }
    if response.skipped_unknown_age > 0 {
        println!(
            "  {} session(s) were left alone because their log's recency could not be read.",
            response.skipped_unknown_age
        );
    }
    if response.contradictions > 0 {
        println!(
            "  {} log contradiction(s) were reported along the way — see `aleph doctor`.",
            response.contradictions
        );
    }
    Ok(())
}
